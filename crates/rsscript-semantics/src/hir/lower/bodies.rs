//! Syntax-to-HIR body, statement, expression, and call lowering.

use super::*;

pub(super) fn build_function_bodies(facts: &BodyFacts) -> HashMap<String, HirFunctionBody> {
    let mut bodies = HashMap::<String, HirFunctionBody>::new();
    for (function_name, block) in &facts.blocks {
        body_entry(&mut bodies, function_name).block = Some(block.clone());
    }
    for binding in &facts.bindings {
        body_entry(&mut bodies, &binding.function_name)
            .bindings
            .push(binding.clone());
    }
    for site in &facts.call_sites {
        body_entry(&mut bodies, &site.function_name)
            .call_sites
            .push(site.clone());
    }
    for field in &facts.field_accesses {
        body_entry(&mut bodies, &field.function_name)
            .field_accesses
            .push(field.clone());
    }
    for event in &facts.effect_events {
        body_entry(&mut bodies, &event.function_name)
            .effect_events
            .push(event.clone());
    }
    for return_fact in &facts.returns {
        body_entry(&mut bodies, &return_fact.function_name)
            .returns
            .push(return_fact.clone());
    }
    bodies
}

pub(super) fn body_entry<'a>(
    bodies: &'a mut HashMap<String, HirFunctionBody>,
    function_name: &str,
) -> &'a mut HirFunctionBody {
    bodies
        .entry(function_name.to_string())
        .or_insert_with(|| HirFunctionBody {
            function_name: function_name.to_string(),
            ..HirFunctionBody::default()
        })
}

#[derive(Default)]
pub(in crate::hir::lower) struct BodyFacts {
    pub(in crate::hir::lower) blocks: HashMap<String, HirBlock>,
    pub(in crate::hir::lower) call_sites: Vec<HirCallSite>,
    pub(in crate::hir::lower) bindings: Vec<HirBinding>,
    pub(in crate::hir::lower) field_accesses: Vec<HirFieldAccess>,
    pub(in crate::hir::lower) effect_events: Vec<HirEffectEvent>,
    pub(in crate::hir::lower) returns: Vec<HirReturn>,
}

pub(super) fn collect_function_body_facts(
    hir: &Hir,
    function: &FunctionDecl,
    facts: &mut BodyFacts,
) {
    let mut value_types = HashMap::new();
    for param in &function.params {
        let param_type = ResolvedType::from_type_ref(&param.ty);
        value_types.insert(param.name.clone(), param_type.clone());
        facts.bindings.push(HirBinding {
            function_name: function.name.clone(),
            name: param.name.clone(),
            kind: HirBindingKind::Param,
            effect: effective_param_effect(param),
            span: param.span.clone(),
            ty: Some(param_type.clone()),
            type_name: Some(param_type.to_string()),
        });
    }
    // Store protocol bounds for receiver-call shorthand resolution.
    // Convention: "__protocol_bound__<TypeParam>" -> "<ProtocolName>"
    for type_param in &function.type_params {
        if let Some(GenericBound::Protocol(protocol)) = &type_param.bound {
            value_types.insert(
                format!("__protocol_bound__{}", type_param.name),
                ResolvedType::named(protocol, []),
            );
        }
    }
    let mut lowering_value_types = value_types.clone();
    facts.blocks.insert(
        function.name.clone(),
        lower_hir_block(
            hir,
            &function.name,
            &function.body,
            &mut lowering_value_types,
        ),
    );
    collect_body_facts_in_block(hir, &function.name, &function.body, &mut value_types, facts);
}

pub(super) fn lower_hir_block(
    hir: &Hir,
    function_name: &str,
    block: &Block,
    value_types: &mut HirValueTypes,
) -> HirBlock {
    let mut statements = Vec::new();
    for statement in &block.statements {
        statements.extend(lower_hir_stmts(hir, function_name, statement, value_types));
    }
    HirBlock {
        statements,
        span: block.span.clone(),
    }
}

pub(super) fn lower_hir_stmts(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    value_types: &mut HirValueTypes,
) -> Vec<HirStmt> {
    match statement {
        Stmt::LetElse(stmt) => {
            let value_type_name = infer_hir_expr_type(hir, &stmt.value, value_types);
            let canonical_value_type = value_type_name.as_ref().map(|ty| {
                let canonical = hir.canonical_type_name(&ty.to_string());
                ResolvedType::named(&canonical, [])
            });
            let binding_type_name =
                match_pattern_binding_type(&stmt.pattern, canonical_value_type.as_ref())
                    .map(|(_, type_name)| type_name);
            let mut statements = vec![HirStmt::Match {
                value: lower_hir_expr(hir, function_name, &stmt.value, value_types),
                scrutinee_effect: None,
                arms: vec![
                    HirMatchArm {
                        pattern: stmt.pattern.clone(),
                        guard: None,
                        body: HirBlock {
                            statements: Vec::new(),
                            span: stmt.span.clone(),
                        },
                        span: stmt.span.clone(),
                    },
                    HirMatchArm {
                        pattern: MatchPattern::Wildcard(stmt.span.clone()),
                        guard: None,
                        body: {
                            let mut else_types = value_types.clone();
                            lower_hir_block(hir, function_name, &stmt.else_body, &mut else_types)
                        },
                        span: stmt.span.clone(),
                    },
                ],
                span: stmt.span.clone(),
            }];
            if !stmt.binding_name.is_empty() {
                if let Some(type_name) = &binding_type_name {
                    value_types.insert(stmt.binding_name.clone(), type_name.clone());
                }
                statements.push(HirStmt::Let {
                    kind: HirBindingKind::ManagedLet,
                    name: stmt.binding_name.clone(),
                    value: None,
                    ty: binding_type_name.clone(),
                    value_ty: value_type_name.clone(),
                    type_name: binding_type_name.map(|ty| ty.to_string()),
                    value_type_name: value_type_name.map(|ty| ty.to_string()),
                    is_async: false,
                    span: stmt.span.clone(),
                });
            }
            statements
        }
        Stmt::TaskGroup(stmt) => {
            let mut body_types = value_types.clone();
            let mut statements =
                lower_hir_block(hir, function_name, &stmt.body, &mut body_types).statements;
            append_task_group_drains(&mut statements);
            statements
        }
        _ => vec![lower_hir_stmt(hir, function_name, statement, value_types)],
    }
}

/// Structured-concurrency drain for a `task_group` body. A `task_group` flattens
/// into its statements (so every checker pass sees the body transparently), but
/// the executable backends must still drain `async let` tasks that the scope
/// spawned and never awaited — leaving the group joins them so background work
/// runs to completion. The compiled backend does this via its scope guard; the
/// reg VM has no such boundary after flattening, so we make the drain explicit
/// here by appending an `await <handle>` for each un-awaited `async let`.
///
/// Only un-awaited handles are drained: the `await` checker (RS0030) consumes an
/// `async let` name the first time it is awaited, so re-awaiting an already-joined
/// handle would both be rejected and be redundant. Discard (`_`) async-lets can
/// never be awaited by name, so they are always drained — and are renamed to
/// unique handles so the appended `await` can reference them (this also fixes
/// multiple `_` handles colliding on the same register/name).
pub(super) fn append_task_group_drains(statements: &mut Vec<HirStmt>) {
    let awaited = collect_awaited_handle_names(statements);
    let mut drains = Vec::new();
    let mut discard_index = 0usize;
    for statement in statements.iter_mut() {
        let HirStmt::Let {
            name,
            value: Some(_),
            is_async: true,
            span,
            ..
        } = statement
        else {
            continue;
        };
        if name == "_" {
            *name = format!("__rss_task_group_discard_{discard_index}");
            discard_index += 1;
        } else if awaited.contains(name) {
            // Already awaited in the body; the handle is consumed.
            continue;
        }
        let span = span.clone();
        drains.push(HirStmt::Expr(HirExpr::Await {
            value: Box::new(HirExpr::Ident {
                name: name.clone(),
                type_name: None,
                span: span.clone(),
            }),
            type_name: None,
            span,
        }));
    }
    statements.extend(drains);
}

/// Names of `async let` handles the body awaits at least once, so the drain can
/// skip them. `await x` and `await x?` (which wraps the ident in `Try`/`Effect`)
/// both count as awaiting `x`.
pub(super) fn collect_awaited_handle_names(statements: &[HirStmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in statements {
        walk_stmt_for_awaits(statement, &mut names);
    }
    names
}

pub(super) fn walk_stmt_for_awaits(statement: &HirStmt, names: &mut HashSet<String>) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                walk_expr_for_awaits(value, names);
            }
        }
        HirStmt::Expr(expr) => walk_expr_for_awaits(expr, names),
        HirStmt::Assign { target, value, .. } => {
            walk_expr_for_awaits(target, names);
            walk_expr_for_awaits(value, names);
        }
        HirStmt::With { resource, body, .. } => {
            walk_expr_for_awaits(resource, names);
            walk_block_for_awaits(body, names);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            walk_expr_for_awaits(condition, names);
            walk_block_for_awaits(then_body, names);
            if let Some(else_body) = else_body {
                walk_block_for_awaits(else_body, names);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                walk_expr_for_awaits(condition, names);
            }
            walk_block_for_awaits(body, names);
        }
        HirStmt::For { iterable, body, .. } => {
            walk_expr_for_awaits(iterable, names);
            walk_block_for_awaits(body, names);
        }
        HirStmt::Match { value, arms, .. } => {
            walk_expr_for_awaits(value, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_for_awaits(guard, names);
                }
                walk_block_for_awaits(&arm.body, names);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                walk_expr_for_awaits(&arm.operation, names);
                walk_block_for_awaits(&arm.body, names);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn walk_block_for_awaits(block: &HirBlock, names: &mut HashSet<String>) {
    for statement in &block.statements {
        walk_stmt_for_awaits(statement, names);
    }
}

pub(super) fn walk_expr_for_awaits(expr: &HirExpr, names: &mut HashSet<String>) {
    match expr {
        HirExpr::Await { value, .. } => {
            if let Some(name) = awaited_handle_name(value) {
                names.insert(name);
            }
            walk_expr_for_awaits(value, names);
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                walk_expr_for_awaits(&field.value, names);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                walk_expr_for_awaits(&entry.key, names);
                walk_expr_for_awaits(&entry.value, names);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                walk_expr_for_awaits(item, names);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            walk_expr_for_awaits(left, names);
            walk_expr_for_awaits(right, names);
        }
        HirExpr::Field { base, .. } => walk_expr_for_awaits(base, names),
        HirExpr::Index { base, index, .. } => {
            walk_expr_for_awaits(base, names);
            walk_expr_for_awaits(index, names);
        }
        HirExpr::Call { receiver, args, .. } => {
            if let Some(receiver) = receiver {
                walk_expr_for_awaits(&receiver.value, names);
            }
            for arg in args {
                walk_expr_for_awaits(&arg.value, names);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Try { value, .. } => walk_expr_for_awaits(value, names),
        HirExpr::Closure { body, .. } => walk_block_for_awaits(body, names),
        HirExpr::Match { value, arms, .. } => {
            walk_expr_for_awaits(value, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_for_awaits(guard, names);
                }
                walk_block_for_awaits(&arm.body, names);
            }
        }
    }
}

/// Peel `Try`/`Effect` wrappers off an awaited operand to recover the handle
/// identifier, e.g. the `x` in `await x?`.
pub(super) fn awaited_handle_name(expr: &HirExpr) -> Option<String> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name.clone()),
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => awaited_handle_name(value),
        _ => None,
    }
}

pub(super) fn lower_hir_stmt(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    value_types: &mut HirValueTypes,
) -> HirStmt {
    match statement {
        Stmt::Let(stmt) => {
            let value_type_name = stmt
                .value
                .as_ref()
                .and_then(|value| infer_hir_expr_type(hir, value, value_types));
            let declared_type_name = stmt
                .type_annotation
                .as_ref()
                .map(ResolvedType::from_type_ref);
            let ty = declared_type_name
                .clone()
                .or_else(|| value_type_name.clone());
            let value = stmt
                .value
                .as_ref()
                .map(|value| lower_hir_expr(hir, function_name, value, value_types));
            if let Some(ty) = &ty {
                value_types.insert(stmt.name.clone(), ty.clone());
            }
            HirStmt::Let {
                kind: hir_binding_kind(stmt.kind),
                name: stmt.name.clone(),
                value,
                ty: ty.clone(),
                value_ty: value_type_name.clone(),
                type_name: ty.map(|ty| ty.to_string()),
                value_type_name: value_type_name.map(|ty| ty.to_string()),
                is_async: stmt.is_async,
                span: stmt.span.clone(),
            }
        }
        Stmt::Return(stmt) => {
            let proof = stmt
                .value
                .as_ref()
                .map_or(HirReturnProof::NoValue, |value| {
                    classify_return_expr(hir, value, value_types)
                });
            HirStmt::Return {
                value: stmt
                    .value
                    .as_ref()
                    .map(|value| lower_hir_expr(hir, function_name, value, value_types)),
                proof,
                span: stmt.span.clone(),
            }
        }
        Stmt::With(stmt) => {
            let resource_type = infer_hir_expr_type(hir, &stmt.resource, value_types);
            let mut body_types = value_types.clone();
            if let Some(resource_type) = &resource_type {
                body_types.insert(stmt.binding.clone(), resource_type.clone());
            }
            HirStmt::With {
                resource: lower_hir_expr(hir, function_name, &stmt.resource, value_types),
                resource_type,
                binding: stmt.binding.clone(),
                body: lower_hir_block(hir, function_name, &stmt.body, &mut body_types),
                span: stmt.span.clone(),
            }
        }
        Stmt::If(stmt) => HirStmt::If {
            condition: lower_hir_expr(hir, function_name, &stmt.condition, value_types),
            then_body: {
                let mut then_types = value_types.clone();
                lower_hir_block(hir, function_name, &stmt.then_body, &mut then_types)
            },
            else_body: stmt.else_body.as_ref().map(|else_body| {
                let mut else_types = value_types.clone();
                lower_hir_block(hir, function_name, else_body, &mut else_types)
            }),
            span: stmt.span.clone(),
        },
        Stmt::Loop(stmt) => HirStmt::Loop {
            condition: stmt
                .condition
                .as_ref()
                .map(|condition| lower_hir_expr(hir, function_name, condition, value_types)),
            body: {
                let mut body_types = value_types.clone();
                lower_hir_block(hir, function_name, &stmt.body, &mut body_types)
            },
            span: stmt.span.clone(),
        },
        Stmt::For(stmt) => {
            let iterable_type = infer_hir_expr_type(hir, &stmt.iterable, value_types);
            let item_type = if stmt.is_async {
                iterable_type.as_ref().and_then(stream_item_type)
            } else {
                iterable_type.as_ref().and_then(list_element_type)
            };
            let mut body_types = value_types.clone();
            if let Some(item_type) = &item_type {
                body_types.insert(stmt.binding.clone(), item_type.clone());
            }
            HirStmt::For {
                binding: stmt.binding.clone(),
                iterable: lower_hir_expr(hir, function_name, &stmt.iterable, value_types),
                iterable_type_name: iterable_type.as_ref().map(ToString::to_string),
                item_type_name: item_type.as_ref().map(ToString::to_string),
                iterable_type,
                item_type,
                is_async: stmt.is_async,
                body: lower_hir_block(hir, function_name, &stmt.body, &mut body_types),
                span: stmt.span.clone(),
            }
        }
        Stmt::Match(stmt) => {
            let value_type = infer_hir_expr_type(hir, &stmt.value, value_types);
            let value = lower_hir_expr(hir, function_name, &stmt.value, value_types);
            let arms = stmt
                .arms
                .iter()
                .map(|arm| {
                    let mut arm_types = value_types.clone();
                    for (binding, type_name) in
                        match_pattern_binding_types(hir, &arm.pattern, value_type.as_ref())
                    {
                        arm_types.insert(binding, type_name);
                    }
                    HirMatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|guard| lower_hir_expr(hir, function_name, guard, &arm_types)),
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirStmt::Match {
                value,
                scrutinee_effect: stmt.scrutinee_effect,
                arms,
                span: stmt.span.clone(),
            }
        }
        Stmt::Select(stmt) => {
            let arms = stmt
                .arms
                .iter()
                .map(|arm| {
                    // The binding observes the *awaited* value of the operation,
                    // so the body sees it with the resolved (unwrapped) type.
                    let binding_type = infer_hir_expr_type(hir, &arm.operation, value_types);
                    let operation = lower_hir_expr(hir, function_name, &arm.operation, value_types);
                    let mut arm_types = value_types.clone();
                    if arm.binding != "_"
                        && let Some(type_name) = binding_type
                    {
                        arm_types.insert(arm.binding.clone(), type_name);
                    }
                    HirSelectArm {
                        binding: arm.binding.clone(),
                        operation,
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirStmt::Select {
                arms,
                span: stmt.span.clone(),
            }
        }
        Stmt::TaskGroup(_) => unreachable!("task-group statements are lowered by lower_hir_stmts"),
        Stmt::LetElse(_) => unreachable!("let-else statements are lowered by lower_hir_stmts"),
        // Controlled assignment is checked at the AST level; in HIR it carries
        // the value expression so the RHS still gets ownership/use analysis, and
        // the lowered target so executable backends know which binding to store.
        Stmt::Assign(stmt) => HirStmt::Assign {
            target: lower_hir_expr(hir, function_name, &stmt.target, value_types),
            value: lower_hir_expr(hir, function_name, &stmt.value, value_types),
            span: stmt.span.clone(),
        },
        Stmt::Expr(expr) => HirStmt::Expr(lower_hir_expr(hir, function_name, expr, value_types)),
        Stmt::Break(span) => HirStmt::Break(span.clone()),
        Stmt::Continue(span) => HirStmt::Continue(span.clone()),
        Stmt::MalformedWith(span)
        | Stmt::MalformedIf(span)
        | Stmt::MalformedLoop(span)
        | Stmt::MalformedFor(span)
        | Stmt::MalformedMatch(span)
        | Stmt::Unknown(span) => HirStmt::Unknown(span.clone()),
    }
}

pub(super) fn lower_hir_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    value_types: &HirValueTypes,
) -> HirExpr {
    match expr {
        // A reference to a top-level `const` is inlined to its literal value: the
        // register VM has no const/global slots, and the literal carries the value
        // to every backend. A local binding of the same name shadows the const.
        Expr::Ident(name, _)
            if !value_types.contains_key(name) && hir.const_values.contains_key(name) =>
        {
            let value = hir.const_values[name].clone();
            lower_hir_expr(hir, function_name, &value, value_types)
        }
        Expr::Ident(name, span) => HirExpr::Ident {
            name: name.clone(),
            type_name: value_types.get(name).map(ToString::to_string),
            span: span.clone(),
        },
        Expr::Number(value, span) => HirExpr::Number {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::String(value, span) => HirExpr::String {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::MultilineString(value, span) => HirExpr::String {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::CharLiteral(value, span) => HirExpr::Char {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::ObjectLiteral { fields, span } => HirExpr::ObjectLiteral {
            fields: fields
                .iter()
                .map(|field| HirObjectLiteralField {
                    name: field.name.clone(),
                    value: lower_hir_expr(hir, function_name, &field.value, value_types),
                    span: field.span.clone(),
                })
                .collect(),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::MapLiteral { entries, span } => HirExpr::MapLiteral {
            entries: entries
                .iter()
                .map(|entry| HirMapLiteralEntry {
                    key: lower_hir_expr(hir, function_name, &entry.key, value_types),
                    value: lower_hir_expr(hir, function_name, &entry.value, value_types),
                    span: entry.span.clone(),
                })
                .collect(),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::ArrayLiteral { items, span } => HirExpr::ArrayLiteral {
            items: items
                .iter()
                .map(|item| lower_hir_expr(hir, function_name, item, value_types))
                .collect(),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => HirExpr::Binary {
            op: *op,
            left: Box::new(lower_hir_expr(hir, function_name, left, value_types)),
            right: Box::new(lower_hir_expr(hir, function_name, right, value_types)),
            span: span.clone(),
        },
        Expr::Field { base, name, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types);
            let base_type_display = base_type.as_ref().map(ToString::to_string);
            let resolved = base_type.as_ref().and_then(|ty| {
                let canonical = hir.canonical_type_name(&ty.to_string());
                let type_info = hir.type_info(&canonical)?;
                let field = type_info.fields.get(name)?;
                Some((type_info, ty, field))
            });
            HirExpr::Field {
                base: Box::new(lower_hir_expr(hir, function_name, base, value_types)),
                name: name.clone(),
                access: HirFieldAccess {
                    function_name: function_name.to_string(),
                    name: name.clone(),
                    span: span.clone(),
                    ty: resolved.map(|(type_info, ty, field)| {
                        substituted_field_type(hir, type_info, ty, field)
                    }),
                    is_handle: resolved
                        .is_some_and(|(_, _, field)| field.is_handle || field.is_weak),
                    is_weak: resolved.is_some_and(|(_, _, field)| field.is_weak),
                    base_ty: base_type.clone(),
                    base_type: base_type_display,
                    type_name: resolved.map(|(type_info, ty, field)| {
                        substituted_field_type(hir, type_info, ty, field).to_string()
                    }),
                },
                span: span.clone(),
            }
        }
        Expr::Index { base, index, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types);
            HirExpr::Index {
                base: Box::new(lower_hir_expr(hir, function_name, base, value_types)),
                index: Box::new(lower_hir_expr(hir, function_name, index, value_types)),
                base_type,
                span: span.clone(),
            }
        }
        Expr::Call { callee, args, span } => {
            lower_hir_call_expr(hir, function_name, expr, callee, args, span, value_types)
        }
        Expr::Effect {
            effect,
            value,
            span,
        } => HirExpr::Effect {
            effect: param_effect_from_data_effect(*effect),
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            events: effect_events_for_expr(function_name, expr),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::Manage { value, span } => {
            let ty = infer_hir_expr_type(hir, expr, value_types);
            HirExpr::Manage {
                value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
                events: effect_events_for_expr(function_name, expr),
                type_name: ty.as_ref().map(ToString::to_string),
                ty,
                span: span.clone(),
            }
        }
        Expr::Spawn { value, span } => HirExpr::Spawn {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::Await { value, span } => HirExpr::Await {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::Try { value, span } => HirExpr::Try {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string()),
            span: span.clone(),
        },
        Expr::Closure {
            params,
            captures,
            explicit,
            body,
            span,
        } => {
            let mut closure_types = value_types.clone();
            HirExpr::Closure {
                params: params.clone(),
                captures: captures
                    .iter()
                    .map(|capture| HirClosureCapture {
                        effect: param_effect_from_data_effect(capture.effect),
                        name: capture.name.clone(),
                        span: capture.span.clone(),
                    })
                    .collect(),
                explicit: *explicit,
                body: lower_hir_block(hir, function_name, body, &mut closure_types),
                span: span.clone(),
            }
        }
        Expr::Match {
            value,
            scrutinee_effect,
            arms,
            span,
            ..
        } => {
            let value_type = infer_hir_expr_type(hir, value, value_types);
            let lowered_value = lower_hir_expr(hir, function_name, value, value_types);
            let mut match_type = None;
            let lowered_arms = arms
                .iter()
                .map(|arm| {
                    let mut arm_types = value_types.clone();
                    for (binding, type_name) in
                        match_pattern_binding_types(hir, &arm.pattern, value_type.as_ref())
                    {
                        arm_types.insert(binding, type_name);
                    }
                    if match_type.is_none() {
                        match_type = infer_closure_return_type(hir, &arm.body, &arm_types);
                    }
                    HirMatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|guard| lower_hir_expr(hir, function_name, guard, &arm_types)),
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirExpr::Match {
                value: Box::new(lowered_value),
                scrutinee_effect: *scrutinee_effect,
                arms: lowered_arms,
                type_name: match_type.map(|ty| ty.to_string()),
                span: span.clone(),
            }
        }
        Expr::Unknown(span) => HirExpr::Unknown(span.clone()),
    }
}

/// Lowers an `Expr::Call`: receiver type inference, call resolution, retain
/// events, and default-parameter synthesis. Extracted from `lower_hir_expr`;
/// `expr` is the original `Expr::Call` node (needed for type inference).
pub(super) fn lower_hir_call_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    callee: &Callee,
    args: &[CallArg],
    span: &Span,
    value_types: &HirValueTypes,
) -> HirExpr {
    let receiver_type = match callee {
        Callee::ReceiverCall { receiver, .. } => infer_hir_expr_type(hir, receiver, value_types),
        _ => None,
    };
    let (resolution, resolved_namespace) = match callee {
        Callee::ReceiverCall { method, .. } => {
            if let Some(receiver_type) = receiver_type.as_ref() {
                hir.resolve_receiver_call_structured(receiver_type, method, value_types)
            } else {
                (CallResolution::Unknown, None)
            }
        }
        _ => (hir.resolve_call(callee), None),
    };
    let events = retain_events_for_call(
        function_name,
        callee,
        args,
        span,
        &resolution,
        hir,
        value_types,
    );
    let type_name = infer_hir_expr_type(hir, expr, value_types).map(|ty| ty.to_string());
    let mut hir_args: Vec<HirCallArg> = args
        .iter()
        .enumerate()
        .map(|(index, arg)| HirCallArg {
            name: arg.name.clone(),
            value: lower_hir_expr(hir, function_name, &arg.value, value_types),
            parameter_index: None,
            evaluation_index: index,
            span: arg.span.clone(),
        })
        .collect();
    let user_variant_fields = match callee {
        Callee::Name(name) => hir.sum_variant_fields(type_root_name(name)),
        _ => None,
    };
    let call_binding = match &resolution {
        CallResolution::Resolved { signature, kind } => {
            let parameter_names = signature
                .params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>();
            let parameter_has_default = signature
                .params
                .iter()
                .map(|parameter| parameter.default.is_some())
                .collect::<Vec<_>>();
            let parameter_allows_shorthand = signature
                .params
                .iter()
                .map(|parameter| {
                    parameter.effect == Some(ParamEffect::Read)
                        || matches!(kind, ResolvedCalleeKind::Constructor { .. })
                })
                .collect::<Vec<_>>();
            let argument_names = hir_args
                .iter()
                .map(|argument| argument.name.as_deref())
                .collect::<Vec<_>>();
            let argument_shorthand_names = hir_args
                .iter()
                .map(|argument| match &argument.value {
                    HirExpr::Ident { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            Some(crate::CallBinding::bind(
                &parameter_names,
                &parameter_has_default,
                &parameter_allows_shorthand,
                &argument_names,
                &argument_shorthand_names,
                usize::from(matches!(callee, Callee::ReceiverCall { .. })),
            ))
        }
        CallResolution::EnumVariant => user_variant_fields.map(|fields| {
            let parameter_names = fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>();
            let parameter_has_default = fields
                .iter()
                .map(|field| field.default.is_some())
                .collect::<Vec<_>>();
            let parameter_allows_shorthand = vec![true; fields.len()];
            let argument_names = hir_args
                .iter()
                .map(|argument| argument.name.as_deref())
                .collect::<Vec<_>>();
            let argument_shorthand_names = hir_args
                .iter()
                .map(|argument| match &argument.value {
                    HirExpr::Ident { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            crate::CallBinding::bind(
                &parameter_names,
                &parameter_has_default,
                &parameter_allows_shorthand,
                &argument_names,
                &argument_shorthand_names,
                0,
            )
        }),
        _ => None,
    };

    // Syntax preserves whether a caller wrote `read`, but the semantic form has
    // exactly one representation: a bare argument for a read parameter is the
    // same read effect as an explicitly wrapped argument. Parameter selection is
    // read only from the shared binding result.
    if let (CallResolution::Resolved { signature, .. }, Some(binding)) =
        (&resolution, &call_binding)
    {
        for (index, arg) in hir_args.iter_mut().enumerate() {
            let Some(bound) = binding.explicit(index) else {
                continue;
            };
            arg.parameter_index = Some(bound.parameter_index);
            arg.evaluation_index = bound.evaluation_index;
            let param = &signature.params[bound.parameter_index];
            if arg.name.is_none()
                && matches!(&arg.value, HirExpr::Ident { name, .. } if *name == param.name)
            {
                arg.name = Some(param.name.clone());
            }
            let actual_type = args
                .get(index)
                .and_then(|source_arg| infer_hir_expr_type(hir, &source_arg.value, value_types));
            if param.effect == Some(ParamEffect::Read) && !hir_expr_already_read(&arg.value) {
                let value = std::mem::replace(&mut arg.value, HirExpr::Unknown(arg.span.clone()));
                arg.value = HirExpr::Effect {
                    effect: ParamEffect::Read,
                    value: Box::new(value),
                    events: Vec::new(),
                    type_name: actual_type
                        .map(|ty| ty.to_string())
                        .or_else(|| Some(param.ty.to_string())),
                    span: arg.span.clone(),
                };
            }
        }
    }
    if matches!(resolution, CallResolution::EnumVariant)
        && let (Some(fields), Some(binding)) = (user_variant_fields, &call_binding)
    {
        for (index, arg) in hir_args.iter_mut().enumerate() {
            let Some(bound) = binding.explicit(index) else {
                continue;
            };
            arg.parameter_index = Some(bound.parameter_index);
            arg.evaluation_index = bound.evaluation_index;
            let field = &fields[bound.parameter_index];
            if arg.name.is_none()
                && matches!(&arg.value, HirExpr::Ident { name, .. } if *name == field.name)
            {
                arg.name = Some(field.name.clone());
            }
        }
    }
    // Fill omitted parameters that declare a default value, so every
    // backend sees a complete call (defaults are desugared once, here).
    if let (CallResolution::Resolved { signature, .. }, Some(binding)) =
        (&resolution, &call_binding)
    {
        for bound in binding.defaults() {
            let param = &signature.params[bound.parameter_index];
            if let Some(default) = &param.default {
                // Defaults execute at the call site, but their names are bound in
                // the declaration environment. Caller locals must not shadow a
                // top-level constant referenced by a default.
                let mut value = lower_hir_expr(hir, function_name, default, &HirValueTypes::new());
                // A non-Copy default is materialized at the call site and
                // bound under the parameter's declared effect. Carry that
                // effect on the synthesized argument so the call-site
                // effect check is satisfied — the effect is reviewed at the
                // declaration (`axes: read List<Int> = ...`), not at the
                // omitted call where the argument is implicit.
                if let Some(effect) = param.effect {
                    value = HirExpr::Effect {
                        effect,
                        value: Box::new(value),
                        events: Vec::new(),
                        type_name: Some(param.ty.to_string()),
                        span: span.clone(),
                    };
                }
                hir_args.push(HirCallArg {
                    name: Some(param.name.clone()),
                    value,
                    parameter_index: Some(bound.parameter_index),
                    evaluation_index: bound.evaluation_index,
                    span: span.clone(),
                });
            }
        }
    }
    if matches!(resolution, CallResolution::EnumVariant)
        && let (Some(fields), Some(binding)) = (user_variant_fields, &call_binding)
    {
        for bound in binding.defaults() {
            let field = &fields[bound.parameter_index];
            if let Some(default) = &field.default {
                hir_args.push(HirCallArg {
                    name: Some(field.name.clone()),
                    value: lower_hir_expr(hir, function_name, default, &HirValueTypes::new()),
                    parameter_index: Some(bound.parameter_index),
                    evaluation_index: bound.evaluation_index,
                    span: span.clone(),
                });
            }
        }
    }
    HirExpr::Call {
        callee: callee.clone(),
        receiver: match callee {
            Callee::ReceiverCall {
                receiver, effect, ..
            } => Some(HirCallReceiver {
                value: Box::new(lower_hir_expr(hir, function_name, receiver, value_types)),
                effect: param_effect_from_data_effect((*effect).unwrap_or(DataEffect::Read)),
                type_name: receiver_type.map(|ty| ty.to_string()),
                resolved_namespace,
            }),
            _ => None,
        },
        args: hir_args,
        type_name,
        resolution,
        events,
        span: span.clone(),
    }
}
