use super::*;

/// A child of an AST [`Stmt`]/[`Expr`] reached during structural descent: either
/// a sub-expression or a sub-block. The child-walkers hand these to a single
/// visitor callback so the callback can hold one mutable borrow of the
/// accumulator it is filling.
pub(super) enum AstChild<'a> {
    Expr(&'a Expr),
    Block(&'a Block),
}

/// Drives the structural recursion for an AST [`Stmt`]: invokes `visit` for each
/// child expression / block in evaluation order. Side effects collected per node
/// live in the callers; this only centralizes the "recurse into the children"
/// skeleton shared by the AST fact collectors. Pure restructuring — order is
/// identical to the hand-written descent it replaces.
pub(super) fn walk_ast_stmt_children(stmt: &Stmt, visit: &mut dyn FnMut(AstChild<'_>)) {
    match stmt {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                visit(AstChild::Expr(value));
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                visit(AstChild::Expr(value));
            }
        }
        Stmt::With(stmt) => {
            visit(AstChild::Expr(&stmt.resource));
            visit(AstChild::Block(&stmt.body));
        }
        Stmt::If(stmt) => {
            visit(AstChild::Expr(&stmt.condition));
            visit(AstChild::Block(&stmt.then_body));
            if let Some(else_body) = &stmt.else_body {
                visit(AstChild::Block(else_body));
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                visit(AstChild::Expr(condition));
            }
            visit(AstChild::Block(&stmt.body));
        }
        Stmt::For(stmt) => {
            visit(AstChild::Expr(&stmt.iterable));
            visit(AstChild::Block(&stmt.body));
        }
        Stmt::TaskGroup(stmt) => {
            visit(AstChild::Block(&stmt.body));
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                visit(AstChild::Expr(&arm.operation));
                visit(AstChild::Block(&arm.body));
            }
        }
        Stmt::Match(stmt) => {
            visit(AstChild::Expr(&stmt.value));
            for arm in &stmt.arms {
                visit(AstChild::Block(&arm.body));
            }
        }
        Stmt::LetElse(stmt) => {
            visit(AstChild::Expr(&stmt.value));
            visit(AstChild::Block(&stmt.else_body));
        }
        Stmt::Assign(stmt) => {
            visit(AstChild::Expr(&stmt.target));
            visit(AstChild::Expr(&stmt.value));
        }
        Stmt::Expr(expr) => visit(AstChild::Expr(expr)),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

/// Drives the structural recursion for an AST [`Expr`]: invokes `visit` for each
/// child expression / block in evaluation order. See [`walk_ast_stmt_children`].
pub(super) fn walk_ast_expr_children(expr: &Expr, visit: &mut dyn FnMut(AstChild<'_>)) {
    match expr {
        Expr::Call { args, .. } => {
            for arg in args {
                visit(AstChild::Expr(&arg.value));
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. }
        | Expr::Field { base: value, .. } => {
            visit(AstChild::Expr(value));
        }
        Expr::Index { base, index, .. }
        | Expr::Binary {
            left: base,
            right: index,
            ..
        } => {
            visit(AstChild::Expr(base));
            visit(AstChild::Expr(index));
        }
        Expr::Closure { body, .. } => visit(AstChild::Block(body)),
        Expr::Match { value, arms, .. } => {
            visit(AstChild::Expr(value));
            for arm in arms {
                visit(AstChild::Block(&arm.body));
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                visit(AstChild::Expr(&entry.key));
                visit(AstChild::Expr(&entry.value));
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn collect_review_map_local_closure_bindings_block(
    block: &Block,
    bindings: &mut BTreeSet<String>,
) {
    for statement in &block.statements {
        collect_review_map_local_closure_bindings_stmt(statement, bindings);
    }
}

pub(super) fn collect_review_map_local_closure_bindings_stmt(
    stmt: &Stmt,
    bindings: &mut BTreeSet<String>,
) {
    if let Stmt::Let(let_stmt) = stmt
        && let_stmt.kind == LetKind::Local
        && matches!(let_stmt.value, Some(Expr::Closure { .. }))
    {
        bindings.insert(let_stmt.name.clone());
    }
    walk_ast_stmt_children(stmt, &mut |child| match child {
        AstChild::Expr(value) => collect_review_map_local_closure_bindings_expr(value, bindings),
        AstChild::Block(block) => collect_review_map_local_closure_bindings_block(block, bindings),
    });
}

pub(super) fn collect_review_map_local_closure_bindings_expr(
    expr: &Expr,
    bindings: &mut BTreeSet<String>,
) {
    walk_ast_expr_children(expr, &mut |child| match child {
        AstChild::Expr(value) => collect_review_map_local_closure_bindings_expr(value, bindings),
        AstChild::Block(block) => collect_review_map_local_closure_bindings_block(block, bindings),
    });
}

pub(super) fn collect_review_map_facts_block(
    block: &Block,
    hir: &Hir,
    callback_params: &BTreeSet<String>,
    local_closure_bindings: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    for statement in &block.statements {
        collect_review_map_facts_stmt(
            statement,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        );
    }
}

pub(super) fn collect_review_map_facts_stmt(
    statement: &Stmt,
    hir: &Hir,
    callback_params: &BTreeSet<String>,
    local_closure_bindings: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    // `Match` interleaves scoped-binding bookkeeping with the descent into each
    // arm, so it cannot route through the shared child-walker; handle it here and
    // return. Every other statement is "record side effects, then recurse into
    // children", which the walker drives.
    if let Stmt::Match(stmt) = statement {
        collect_review_map_facts_expr(
            &stmt.value,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        );
        let value_type = review_map_expr_type_name_with_facts(&stmt.value, hir, &facts.value_types);
        for arm in &stmt.arms {
            let scoped_binding = review_map_match_binding_type(&arm.pattern, value_type.as_deref());
            let previous = scoped_binding
                .as_ref()
                .and_then(|(binding, _)| facts.value_types.get(binding).cloned());
            if let Some((binding, type_name)) = &scoped_binding {
                facts.value_types.insert(binding.clone(), type_name.clone());
            }
            collect_review_map_facts_block(
                &arm.body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            if let Some((binding, _)) = scoped_binding {
                if let Some(previous) = previous {
                    facts.value_types.insert(binding, previous);
                } else {
                    facts.value_types.remove(&binding);
                }
            }
        }
        return;
    }

    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                facts.has_local = true;
            }
            if let Some(ty) = &stmt.type_annotation {
                facts
                    .value_types
                    .insert(stmt.name.clone(), type_ref_display_name(ty));
            }
            if let Some(value) = &stmt.value
                && !facts.value_types.contains_key(&stmt.name)
                && let Some(type_name) =
                    review_map_expr_type_name_with_facts(value, hir, &facts.value_types)
            {
                facts.value_types.insert(stmt.name.clone(), type_name);
            }
        }
        Stmt::With(_) => facts.has_with = true,
        _ => {}
    }

    walk_ast_stmt_children(statement, &mut |child| match child {
        AstChild::Expr(value) => collect_review_map_facts_expr(
            value,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        ),
        AstChild::Block(block) => collect_review_map_facts_block(
            block,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        ),
    });
}

pub(super) fn review_map_expr_type_name(expr: &Expr, hir: &Hir) -> Option<String> {
    match expr {
        Expr::Call { callee, .. } => match hir.resolve_call(callee) {
            CallResolution::Resolved { signature, kind } => {
                if let ResolvedCalleeKind::Constructor { .. } = kind
                    && let Callee::Qualified { namespace, .. } = callee
                {
                    return Some(namespace.clone());
                }
                signature.return_type.clone()
            }
            CallResolution::EnumVariant
            | CallResolution::Ambiguous { .. }
            | CallResolution::Unknown => None,
        },
        Expr::Effect { value, .. } => review_map_expr_type_name(value, hir),
        Expr::Try { value, .. } => {
            review_map_expr_type_name(value, hir).and_then(|ty| result_ok_type_name(&ty))
        }
        Expr::Await { value, .. } => review_map_expr_type_name(value, hir),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some("String".to_string()),
        Expr::CharLiteral(_, _) => Some("Char".to_string()),
        Expr::Number(value, _) => Some(crate::hir::number_literal_type_name(value).to_string()),
        Expr::Ident(name, _) if matches!(name.as_str(), "true" | "false") => {
            Some("Bool".to_string())
        }
        Expr::Ident(name, _) => hir.sum_type_for_variant(name).map(str::to_string),
        Expr::ArrayLiteral { items, .. } => items
            .first()
            .and_then(|item| review_map_expr_type_name(item, hir))
            .map(|item| format!("List<{item}>"))
            .or_else(|| Some("List".to_string())),
        Expr::MapLiteral { .. } => Some("MapLiteral".to_string()),
        Expr::ObjectLiteral { .. } => Some("JsonLiteral".to_string()),
        _ => None,
    }
}

pub(super) fn collect_review_map_facts_expr(
    expr: &Expr,
    hir: &Hir,
    callback_params: &BTreeSet<String>,
    local_closure_bindings: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    // Record this node's contribution to the facts, then recurse into its
    // children via the shared walker. The descent order is unchanged: every arm
    // below previously recursed into exactly the same children after its side
    // effects, which the walker now drives.
    match expr {
        Expr::Call { callee, args, span } => {
            if is_resource_pool_callee(callee) {
                facts.has_resource_pool = true;
            }
            if is_capability_from_callee(callee) {
                facts.has_capability_object = true;
            }
            if let Some(callback) = review_map_callback_call(callee, callback_params) {
                facts.callback_calls.insert(callback.to_string());
            } else if review_map_local_closure_call(callee, local_closure_bindings).is_none() {
                let resolution = match callee {
                    Callee::ReceiverCall {
                        receiver,
                        method,
                        effect,
                    } => {
                        let receiver_label = review_expr_label(receiver);
                        if let Some(receiver_type) =
                            review_map_expr_type_name_with_facts(receiver, hir, &facts.value_types)
                        {
                            let (resolution, namespace) = hir.resolve_receiver_call(
                                &receiver_type,
                                method,
                                &facts.value_types,
                            );
                            if capability_protocol_name(&receiver_type)
                                .is_some_and(|protocol| namespace.as_deref() == Some(protocol))
                            {
                                facts.has_dynamic_protocol_dispatch = true;
                            }
                            facts.receiver_calls.push(ReviewMapReceiverCall {
                                line: span.line,
                                column: span.column,
                                source: format!(
                                    "{} {receiver_label}.{method}",
                                    (*effect).map(|e| e.as_str()).unwrap_or("read")
                                ),
                                canonical_callee: namespace
                                    .map(|namespace| format!("{namespace}.{method}"))
                                    .unwrap_or_else(|| format!("<unresolved>.{method}")),
                                self_effect: (*effect)
                                    .map(|e| e.as_str())
                                    .unwrap_or("read")
                                    .to_string(),
                                resolution: receiver_call_resolution_label(&resolution).to_string(),
                            });
                            resolution
                        } else {
                            facts.receiver_calls.push(ReviewMapReceiverCall {
                                line: span.line,
                                column: span.column,
                                source: format!(
                                    "{} {receiver_label}.{method}",
                                    (*effect).map(|e| e.as_str()).unwrap_or("read")
                                ),
                                canonical_callee: format!("<unresolved>.{method}"),
                                self_effect: (*effect)
                                    .map(|e| e.as_str())
                                    .unwrap_or("read")
                                    .to_string(),
                                resolution: "unknown".to_string(),
                            });
                            CallResolution::Unknown
                        }
                    }
                    _ => hir.resolve_call(callee),
                };
                match resolution {
                    CallResolution::Resolved { signature, kind } => {
                        collect_call_boundary_facts(&signature, facts);
                        if is_capability_protocol_call(callee, args, &facts.value_types) {
                            facts.has_dynamic_protocol_dispatch = true;
                        }
                        if kind == ResolvedCalleeKind::UserFunction {
                            facts.user_calls.insert(function_sig_key(&signature));
                        }
                    }
                    CallResolution::Ambiguous { .. } | CallResolution::Unknown => {
                        facts.unresolved_calls.insert(review_callee_display(callee));
                    }
                    CallResolution::EnumVariant => {}
                }
            }
        }
        Expr::Effect { effect, .. } => match effect {
            DataEffect::Mut => facts.has_mut = true,
            DataEffect::Take => facts.has_take = true,
            DataEffect::Read => {}
        },
        Expr::Manage { .. } => facts.has_manage = true,
        Expr::Spawn { value, .. } => {
            facts.has_spawn = true;
            collect_spawn_capture_names(value, &mut facts.spawn_captures);
        }
        Expr::Await { .. } => facts.has_await = true,
        Expr::Try { .. } => facts.has_error_boundary = true,
        Expr::Closure {
            captures,
            declared_effects,
            explicit,
            ..
        } => {
            if *explicit {
                for capture in captures {
                    facts.explicit_closure_contracts.insert(format!(
                        "captures {} `{}`",
                        effect_label(capture.effect),
                        capture.name
                    ));
                }
                if !declared_effects.is_empty() {
                    facts
                        .explicit_closure_contracts
                        .insert(format!("effects({})", declared_effects.join(", ")));
                }
            }
        }
        Expr::Match { .. }
        | Expr::Binary { .. }
        | Expr::Field { .. }
        | Expr::Index { .. }
        | Expr::MapLiteral { .. }
        | Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }

    walk_ast_expr_children(expr, &mut |child| match child {
        AstChild::Expr(value) => collect_review_map_facts_expr(
            value,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        ),
        AstChild::Block(block) => collect_review_map_facts_block(
            block,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        ),
    });
}

pub(super) fn collect_call_boundary_facts(signature: &HirFunctionSig, facts: &mut ReviewMapFacts) {
    let callee = function_sig_key(signature);
    if signature.effects.iter().any(|effect| effect == "native") {
        facts.native_calls.insert(callee.clone());
    }
    if signature.effects.iter().any(|effect| effect == "unsafe") {
        facts.unsafe_calls.insert(callee);
    }
}

pub(super) fn review_map_callback_call<'a>(
    callee: &Callee,
    callback_params: &'a BTreeSet<String>,
) -> Option<&'a str> {
    match callee {
        Callee::Name(name) => callback_params.get(name).map(String::as_str),
        Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
    }
}

pub(super) fn review_map_local_closure_call<'a>(
    callee: &Callee,
    local_closure_bindings: &'a BTreeSet<String>,
) -> Option<&'a str> {
    match callee {
        Callee::Name(name) => local_closure_bindings.get(name).map(String::as_str),
        Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
    }
}

pub(super) fn collect_spawn_capture_names(expr: &Expr, captures: &mut BTreeSet<String>) {
    match expr {
        Expr::Ident(name, _) => {
            captures.insert(name.clone());
        }
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            collect_spawn_capture_names(value, captures);
        }
        Expr::Manage { .. } => {}
        Expr::Call { args, .. } => {
            for arg in args {
                collect_spawn_capture_names(&arg.value, captures);
            }
        }
        Expr::Field { base, name, .. } => {
            if let Some(base_name) = spawn_capture_path(base) {
                captures.insert(format!("{base_name}.{name}"));
            } else {
                collect_spawn_capture_names(base, captures);
            }
        }
        Expr::Index { base, index, .. } => {
            collect_spawn_capture_names(base, captures);
            collect_spawn_capture_names(index, captures);
        }
        Expr::Binary { left, right, .. } => {
            collect_spawn_capture_names(left, captures);
            collect_spawn_capture_names(right, captures);
        }
        Expr::Match { value, arms, .. } => {
            collect_spawn_capture_names(value, captures);
            for arm in arms {
                for statement in &arm.body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Expr::Spawn { value, .. } => collect_spawn_capture_names(value, captures),
        Expr::Await { value, .. } => collect_spawn_capture_names(value, captures),
        Expr::Closure { body, .. } => {
            for statement in &body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_spawn_capture_names(&entry.key, captures);
                collect_spawn_capture_names(&entry.value, captures);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn collect_spawn_capture_names_from_stmt(stmt: &Stmt, captures: &mut BTreeSet<String>) {
    match stmt {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_spawn_capture_names(value, captures);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_spawn_capture_names(value, captures);
            }
        }
        Stmt::Assign(stmt) => {
            collect_spawn_capture_names(&stmt.target, captures);
            collect_spawn_capture_names(&stmt.value, captures);
        }
        Stmt::Expr(value) => collect_spawn_capture_names(value, captures),
        Stmt::With(stmt) => {
            collect_spawn_capture_names(&stmt.resource, captures);
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::If(stmt) => {
            collect_spawn_capture_names(&stmt.condition, captures);
            for statement in &stmt.then_body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
            if let Some(else_body) = &stmt.else_body {
                for statement in &else_body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_spawn_capture_names(condition, captures);
            }
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::For(stmt) => {
            collect_spawn_capture_names(&stmt.iterable, captures);
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::TaskGroup(stmt) => {
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_spawn_capture_names(&arm.operation, captures);
                for statement in &arm.body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Stmt::Match(stmt) => {
            collect_spawn_capture_names(&stmt.value, captures);
            for arm in &stmt.arms {
                for statement in &arm.body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Stmt::LetElse(stmt) => {
            collect_spawn_capture_names(&stmt.value, captures);
            for statement in &stmt.else_body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

pub(super) fn spawn_capture_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => {
            spawn_capture_path(base).map(|base| format!("{base}.{name}"))
        }
        Expr::Effect { value, .. } | Expr::Try { value, .. } => spawn_capture_path(value),
        Expr::Manage { .. }
        | Expr::MapLiteral { .. }
        | Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Spawn { .. }
        | Expr::Await { .. }
        | Expr::Index { .. }
        | Expr::Call { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Match { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}
