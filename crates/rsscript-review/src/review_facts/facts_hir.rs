use super::*;

/// A child of a HIR [`HirStmt`]/[`HirExpr`] reached during structural descent.
/// See [`AstChild`]; the single-visitor shape lets the callback hold one mutable
/// borrow of the accumulator.
pub(super) enum HirChild<'a> {
    Expr(&'a HirExpr),
    Block(&'a HirBlock),
}

/// Drives the default structural recursion for a [`HirStmt`]: invokes `visit` for
/// each child expression / block in evaluation order. Nodes whose descent the HIR
/// fact collector specializes (closure-valued `Let`, `Call` with noescape closure
/// args, `Closure`) are handled by the caller and do not route through here. Pure
/// restructuring — order matches the hand-written descent it replaces.
pub(super) fn walk_hir_stmt_children(stmt: &HirStmt, visit: &mut dyn FnMut(HirChild<'_>)) {
    match stmt {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                visit(HirChild::Expr(value));
            }
        }
        HirStmt::With { resource, body, .. } => {
            visit(HirChild::Expr(resource));
            visit(HirChild::Block(body));
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            visit(HirChild::Expr(condition));
            visit(HirChild::Block(then_body));
            if let Some(else_body) = else_body {
                visit(HirChild::Block(else_body));
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                visit(HirChild::Expr(condition));
            }
            visit(HirChild::Block(body));
        }
        HirStmt::For { iterable, body, .. } => {
            visit(HirChild::Expr(iterable));
            visit(HirChild::Block(body));
        }
        HirStmt::Match { value, arms, .. } => {
            visit(HirChild::Expr(value));
            for arm in arms {
                visit(HirChild::Block(&arm.body));
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                visit(HirChild::Expr(&arm.operation));
                visit(HirChild::Block(&arm.body));
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => visit(HirChild::Expr(expr)),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

/// Drives the default structural recursion for a [`HirExpr`]: invokes `visit` for
/// each child expression / block in evaluation order. See [`walk_hir_stmt_children`].
pub(super) fn walk_hir_expr_children(expr: &HirExpr, visit: &mut dyn FnMut(HirChild<'_>)) {
    match expr {
        HirExpr::Binary { left, right, .. } => {
            visit(HirChild::Expr(left));
            visit(HirChild::Expr(right));
        }
        HirExpr::Field { base, .. } => visit(HirChild::Expr(base)),
        HirExpr::Index { base, index, .. } => {
            visit(HirChild::Expr(base));
            visit(HirChild::Expr(index));
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                visit(HirChild::Expr(&arg.value));
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            visit(HirChild::Expr(value));
        }
        HirExpr::Closure { body, .. } => visit(HirChild::Block(body)),
        HirExpr::Match { value, arms, .. } => {
            visit(HirChild::Expr(value));
            for arm in arms {
                visit(HirChild::Block(&arm.body));
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                visit(HirChild::Expr(&entry.key));
                visit(HirChild::Expr(&entry.value));
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn collect_review_map_hir_facts_block(
    block: &HirBlock,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    for statement in &block.statements {
        collect_review_map_hir_facts_stmt(statement, local_bindings, facts);
    }
}

pub(super) fn function_sig_key(signature: &HirFunctionSig) -> String {
    if let Some(namespace) = &signature.namespace {
        format!("{namespace}.{}", signature.name)
    } else {
        signature.name.clone()
    }
}

pub(super) fn collect_review_map_hir_facts_stmt(
    statement: &HirStmt,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    // Closure-valued `Let`s specialize their descent (managed lets also harvest
    // capture names, and both recurse into the closure body rather than treating
    // the value as a plain expression). Everything else recurses via the shared
    // walker, which reproduces the original child order exactly.
    match statement {
        HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(HirExpr::Closure { body, .. }),
            ..
        } => {
            collect_managed_closure_capture_names(body, local_bindings, facts);
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirStmt::Let {
            kind: HirBindingKind::LocalLet,
            value: Some(HirExpr::Closure { body, .. }),
            ..
        } => {
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        _ => walk_hir_stmt_children(statement, &mut |child| match child {
            HirChild::Expr(value) => {
                collect_review_map_hir_facts_expr(value, local_bindings, facts)
            }
            HirChild::Block(block) => {
                collect_review_map_hir_facts_block(block, local_bindings, facts)
            }
        }),
    }
}

pub(super) fn collect_review_map_hir_facts_expr(
    expr: &HirExpr,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    if hir_expr_writes_through_handle_field(expr) {
        facts.has_handle_field_write = true;
    }
    if hir_expr_writes_to_managed_state(expr, local_bindings) {
        facts.has_managed_state_write = true;
    }
    // `Call` (per-argument noescape closure descent) and `Closure` (capture-name
    // harvesting) specialize their descent and are handled here; every other node
    // recurses via the shared walker, preserving the original child order.
    match expr {
        HirExpr::Call {
            args, resolution, ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                collect_call_boundary_facts(signature, facts);
            }
            for (index, arg) in args.iter().enumerate() {
                if let Some(body) = noescape_call_closure_body(arg, index, resolution) {
                    collect_review_map_hir_facts_block(body, local_bindings, facts);
                } else {
                    collect_review_map_hir_facts_expr(&arg.value, local_bindings, facts);
                }
            }
        }
        HirExpr::Closure { body, .. } => {
            collect_managed_closure_capture_names(body, local_bindings, facts);
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        _ => walk_hir_expr_children(expr, &mut |child| match child {
            HirChild::Expr(value) => {
                collect_review_map_hir_facts_expr(value, local_bindings, facts)
            }
            HirChild::Block(block) => {
                collect_review_map_hir_facts_block(block, local_bindings, facts)
            }
        }),
    }
}

pub(super) fn noescape_call_closure_body<'a>(
    arg: &'a rsscript_semantics::hir::HirCallArg,
    index: usize,
    resolution: &CallResolution,
) -> Option<&'a HirBlock> {
    if !call_arg_is_noescape_param(arg, index, resolution) {
        return None;
    }
    hir_closure_body(&arg.value)
}

pub(super) fn call_arg_is_noescape_param(
    arg: &rsscript_semantics::hir::HirCallArg,
    index: usize,
    resolution: &CallResolution,
) -> bool {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return false;
    };
    let Some(param) = arg
        .name
        .as_ref()
        .and_then(|name| signature.params.iter().find(|param| param.name == *name))
        .or_else(|| signature.params.get(index))
    else {
        return false;
    };
    param.ty.qualifiers.noescape && param.ty.is_function()
}

pub(super) fn hir_closure_body(expr: &HirExpr) -> Option<&HirBlock> {
    match expr {
        HirExpr::Closure { body, .. } => Some(body),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => hir_closure_body(value),
        _ => None,
    }
}

pub(super) fn collect_managed_closure_capture_names(
    body: &HirBlock,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    let mut closure_locals = BTreeSet::new();
    collect_managed_closure_capture_names_block(body, local_bindings, &mut closure_locals, facts);
}

pub(super) fn collect_managed_closure_capture_names_block(
    block: &HirBlock,
    local_bindings: &BTreeSet<&str>,
    closure_locals: &mut BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let { name, value, .. } => {
                if let Some(value) = value {
                    collect_managed_closure_capture_names_expr(
                        value,
                        local_bindings,
                        closure_locals,
                        facts,
                    );
                }
                closure_locals.insert(name.clone());
            }
            HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_managed_closure_capture_names_expr(
                    value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                collect_managed_closure_capture_names_expr(
                    resource,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                closure_locals.insert(binding.clone());
                collect_managed_closure_capture_names_block(
                    body,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                closure_locals.remove(binding);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_managed_closure_capture_names_expr(
                    condition,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                let mut then_locals = closure_locals.clone();
                collect_managed_closure_capture_names_block(
                    then_body,
                    local_bindings,
                    &mut then_locals,
                    facts,
                );
                if let Some(else_body) = else_body {
                    let mut else_locals = closure_locals.clone();
                    collect_managed_closure_capture_names_block(
                        else_body,
                        local_bindings,
                        &mut else_locals,
                        facts,
                    );
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_managed_closure_capture_names_expr(
                        condition,
                        local_bindings,
                        closure_locals,
                        facts,
                    );
                }
                let mut body_locals = closure_locals.clone();
                collect_managed_closure_capture_names_block(
                    body,
                    local_bindings,
                    &mut body_locals,
                    facts,
                );
            }
            HirStmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                collect_managed_closure_capture_names_expr(
                    iterable,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                let mut body_locals = closure_locals.clone();
                body_locals.insert(binding.clone());
                collect_managed_closure_capture_names_block(
                    body,
                    local_bindings,
                    &mut body_locals,
                    facts,
                );
            }
            HirStmt::Match { value, arms, .. } => {
                collect_managed_closure_capture_names_expr(
                    value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                for arm in arms {
                    let mut arm_locals = closure_locals.clone();
                    for binding in arm.pattern.binding_names() {
                        arm_locals.insert(binding.to_string());
                    }
                    collect_managed_closure_capture_names_block(
                        &arm.body,
                        local_bindings,
                        &mut arm_locals,
                        facts,
                    );
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_managed_closure_capture_names_expr(
                        &arm.operation,
                        local_bindings,
                        closure_locals,
                        facts,
                    );
                    let mut arm_locals = closure_locals.clone();
                    if arm.binding != "_" {
                        arm_locals.insert(arm.binding.clone());
                    }
                    collect_managed_closure_capture_names_block(
                        &arm.body,
                        local_bindings,
                        &mut arm_locals,
                        facts,
                    );
                }
            }
            HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn collect_managed_closure_capture_names_expr(
    expr: &HirExpr,
    local_bindings: &BTreeSet<&str>,
    closure_locals: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    if let Some((root, path)) = hir_capture_path(expr)
        && !local_bindings.contains(root)
        && !closure_locals.contains(root)
    {
        facts.managed_closure_captures.insert(path);
        return;
    }

    match expr {
        HirExpr::Binary { left, right, .. } => {
            collect_managed_closure_capture_names_expr(left, local_bindings, closure_locals, facts);
            collect_managed_closure_capture_names_expr(
                right,
                local_bindings,
                closure_locals,
                facts,
            );
        }
        HirExpr::Index { base, index, .. } => {
            collect_managed_closure_capture_names_expr(base, local_bindings, closure_locals, facts);
            collect_managed_closure_capture_names_expr(
                index,
                local_bindings,
                closure_locals,
                facts,
            );
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_managed_closure_capture_names_expr(
                    &arg.value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_managed_closure_capture_names_expr(
                value,
                local_bindings,
                closure_locals,
                facts,
            );
        }
        HirExpr::Closure { body, .. } => {
            let mut nested_locals = closure_locals.clone();
            collect_managed_closure_capture_names_block(
                body,
                local_bindings,
                &mut nested_locals,
                facts,
            );
        }
        HirExpr::Match { value, arms, .. } => {
            collect_managed_closure_capture_names_expr(
                value,
                local_bindings,
                closure_locals,
                facts,
            );
            for arm in arms {
                let mut arm_locals = closure_locals.clone();
                collect_managed_closure_capture_names_block(
                    &arm.body,
                    local_bindings,
                    &mut arm_locals,
                    facts,
                );
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_managed_closure_capture_names_expr(
                    &entry.key,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                collect_managed_closure_capture_names_expr(
                    &entry.value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
            }
        }
        HirExpr::Field { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn hir_capture_path(expr: &HirExpr) -> Option<(&str, String)> {
    match expr {
        HirExpr::Ident { name, .. } => Some((name.as_str(), name.clone())),
        HirExpr::Field { base, name, .. } => {
            let (root, path) = hir_capture_path(base)?;
            Some((root, format!("{path}.{name}")))
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => hir_capture_path(value),
        _ => None,
    }
}

pub(super) fn hir_expr_writes_to_managed_state(
    expr: &HirExpr,
    local_bindings: &BTreeSet<&str>,
) -> bool {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Mut | ParamEffect::Take,
            value,
            ..
        } => hir_place_path_root(value).is_some_and(|root| !local_bindings.contains(root)),
        _ => false,
    }
}

pub(super) fn hir_expr_writes_through_handle_field(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Mut | ParamEffect::Take,
            value,
            ..
        } => hir_place_path_crosses_handle_field(value),
        _ => false,
    }
}

pub(super) fn hir_place_path_root(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Field { base, .. } | HirExpr::Index { base, .. } => hir_place_path_root(base),
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => hir_place_path_root(value),
        HirExpr::Binary { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Call { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn hir_place_path_crosses_handle_field(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Field { base, access, .. } => {
            access.is_handle || hir_place_path_crosses_handle_field(base)
        }
        HirExpr::Index { base, .. }
        | HirExpr::Manage { value: base, .. }
        | HirExpr::Spawn { value: base, .. }
        | HirExpr::Await { value: base, .. } => hir_place_path_crosses_handle_field(base),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            hir_place_path_crosses_handle_field(value)
        }
        HirExpr::Ident { .. } => false,
        HirExpr::Binary { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Call { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => false,
    }
}
