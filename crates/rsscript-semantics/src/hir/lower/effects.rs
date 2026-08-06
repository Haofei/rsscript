//! Body fact collection for effects, retains, resources, and feature coverage.

use super::*;

pub(super) fn effect_events_for_expr(function_name: &str, expr: &Expr) -> Vec<HirEffectEvent> {
    let event = match expr {
        Expr::Manage { value, span } => {
            let Some((binding_name, value_span)) = direct_move_binding(value) else {
                return Vec::new();
            };
            HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Manage,
                binding_name,
                span: span.clone(),
                value_span,
            }
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            let Some((binding_name, value_span)) = direct_move_binding(value) else {
                return Vec::new();
            };
            HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Take,
                binding_name,
                span: span.clone(),
                value_span,
            }
        }
        Expr::Effect { .. } => return Vec::new(),
        _ => return Vec::new(),
    };
    vec![event]
}

pub(super) fn retain_events_for_call(
    function_name: &str,
    callee: &Callee,
    args: &[rsscript_syntax::ast::CallArg],
    call_span: &Span,
    resolution: &CallResolution,
    hir: &Hir,
    value_types: &HirValueTypes,
) -> Vec<HirEffectEvent> {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return Vec::new();
    };
    if signature.retained_params.is_empty() {
        return Vec::new();
    }

    let positional_offset = usize::from(matches!(callee, Callee::ReceiverCall { .. }));
    args.iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            let param = arg
                .name
                .as_deref()
                .and_then(|name| signature.params.iter().find(|param| param.name == name))
                .or_else(|| signature.params.get(index + positional_offset))?;
            let name = &param.name;
            if !signature.retained_params.contains(name) {
                return None;
            }
            let retained =
                direct_effect_retained_binding(&arg.value, hir, value_types).or_else(|| {
                    (param.effect == Some(ParamEffect::Read))
                        .then(|| retained_inline_binding(&arg.value, hir, value_types))
                        .flatten()
                });
            let (binding_name, value_span) = retained?;
            Some(HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Retain {
                    callee: callee_display(callee),
                    param: name.clone(),
                },
                binding_name,
                span: call_span.clone(),
                value_span,
            })
        })
        .collect()
}

pub(super) fn collect_body_facts_in_block(
    hir: &Hir,
    function_name: &str,
    block: &Block,
    value_types: &mut HirValueTypes,
    facts: &mut BodyFacts,
) {
    for statement in &block.statements {
        collect_body_facts_in_stmt(hir, function_name, statement, value_types, facts);
    }
}

pub(super) fn collect_body_facts_in_stmt(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    value_types: &mut HirValueTypes,
    facts: &mut BodyFacts,
) {
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
            facts.bindings.push(HirBinding {
                function_name: function_name.to_string(),
                name: stmt.name.clone(),
                kind: hir_binding_kind(stmt.kind),
                effect: None,
                span: stmt.span.clone(),
                ty: ty.clone(),
                type_name: ty.clone().map(|ty| ty.to_string()),
            });
            if let Some(ty) = ty {
                value_types.insert(stmt.name.clone(), ty);
            }
            if let Some(value) = &stmt.value {
                collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                facts.returns.push(HirReturn {
                    function_name: function_name.to_string(),
                    span: value.span().clone(),
                    proof: classify_return_expr(hir, value, value_types),
                });
                collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
            } else {
                facts.returns.push(HirReturn {
                    function_name: function_name.to_string(),
                    span: stmt.span.clone(),
                    proof: HirReturnProof::NoValue,
                });
            }
        }
        Stmt::With(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.resource, value_types, facts);
            let resource_type = infer_hir_expr_type(hir, &stmt.resource, value_types);
            let mut body_types = value_types.clone();
            if let Some(resource_type) = resource_type {
                body_types.insert(stmt.binding.clone(), resource_type);
            }
            collect_body_facts_in_block(hir, function_name, &stmt.body, &mut body_types, facts);
        }
        Stmt::If(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.condition, value_types, facts);
            collect_body_facts_in_block(hir, function_name, &stmt.then_body, value_types, facts);
            if let Some(else_body) = &stmt.else_body {
                collect_body_facts_in_block(hir, function_name, else_body, value_types, facts);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_body_facts_in_expr(hir, function_name, condition, value_types, facts);
            }
            collect_body_facts_in_block(hir, function_name, &stmt.body, value_types, facts);
        }
        Stmt::For(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.iterable, value_types, facts);
            let iterable_type = infer_hir_expr_type(hir, &stmt.iterable, value_types);
            let item_type = if stmt.is_async {
                iterable_type.as_ref().and_then(stream_item_type)
            } else {
                iterable_type.as_ref().and_then(list_element_type)
            };
            let mut body_types = value_types.clone();
            if let Some(item_type) = item_type {
                facts.bindings.push(HirBinding {
                    function_name: function_name.to_string(),
                    name: stmt.binding.clone(),
                    kind: HirBindingKind::ManagedLet,
                    effect: None,
                    span: stmt.span.clone(),
                    ty: Some(item_type.clone()),
                    type_name: Some(item_type.to_string()),
                });
                body_types.insert(stmt.binding.clone(), item_type);
            }
            collect_body_facts_in_block(hir, function_name, &stmt.body, &mut body_types, facts);
        }
        Stmt::TaskGroup(stmt) => {
            let mut body_types = value_types.clone();
            collect_body_facts_in_block(hir, function_name, &stmt.body, &mut body_types, facts);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_body_facts_in_expr(hir, function_name, &arm.operation, value_types, facts);
                let binding_type = infer_hir_expr_type(hir, &arm.operation, value_types);
                let mut arm_types = value_types.clone();
                if arm.binding != "_"
                    && let Some(type_name) = binding_type
                {
                    facts.bindings.push(HirBinding {
                        function_name: function_name.to_string(),
                        name: arm.binding.clone(),
                        kind: HirBindingKind::ManagedLet,
                        effect: None,
                        span: arm.span.clone(),
                        ty: Some(type_name.clone()),
                        type_name: Some(type_name.to_string()),
                    });
                    arm_types.insert(arm.binding.clone(), type_name);
                }
                collect_body_facts_in_block(hir, function_name, &arm.body, &mut arm_types, facts);
            }
        }
        Stmt::Match(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
            let value_type = infer_hir_expr_type(hir, &stmt.value, value_types);
            for arm in &stmt.arms {
                let mut arm_types = value_types.clone();
                for (binding, type_name) in
                    match_pattern_binding_types(hir, &arm.pattern, value_type.as_ref())
                {
                    facts.bindings.push(HirBinding {
                        function_name: function_name.to_string(),
                        name: binding.clone(),
                        kind: HirBindingKind::ManagedLet,
                        effect: None,
                        span: arm.span.clone(),
                        ty: Some(type_name.clone()),
                        type_name: Some(type_name.to_string()),
                    });
                    arm_types.insert(binding, type_name);
                }
                collect_body_facts_in_block(hir, function_name, &arm.body, &mut arm_types, facts);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
            let mut else_types = value_types.clone();
            collect_body_facts_in_block(
                hir,
                function_name,
                &stmt.else_body,
                &mut else_types,
                facts,
            );
            if let Some((binding, type_name)) = match_pattern_binding_type(
                &stmt.pattern,
                infer_hir_expr_type(hir, &stmt.value, value_types).as_ref(),
            ) {
                facts.bindings.push(HirBinding {
                    function_name: function_name.to_string(),
                    name: binding.clone(),
                    kind: HirBindingKind::ManagedLet,
                    effect: None,
                    span: stmt.span.clone(),
                    ty: Some(type_name.clone()),
                    type_name: Some(type_name.to_string()),
                });
                value_types.insert(binding, type_name);
            }
        }
        Stmt::Assign(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.target, value_types, facts);
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
        }
        Stmt::Expr(expr) => {
            collect_body_facts_in_expr(hir, function_name, expr, value_types, facts);
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

pub(super) fn collect_body_facts_in_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    value_types: &mut HirValueTypes,
    facts: &mut BodyFacts,
) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_body_facts_in_expr(hir, function_name, left, value_types, facts);
            collect_body_facts_in_expr(hir, function_name, right, value_types, facts);
        }
        Expr::Call { callee, args, span } => {
            let resolution = match callee {
                Callee::ReceiverCall {
                    receiver, method, ..
                } => {
                    if let Some(receiver_type) = infer_hir_expr_type(hir, receiver, value_types) {
                        let (res, _namespace) = hir.resolve_receiver_call_structured(
                            &receiver_type,
                            method,
                            value_types,
                        );
                        res
                    } else {
                        CallResolution::Unknown
                    }
                }
                _ => hir.resolve_call(callee),
            };
            facts.call_sites.push(HirCallSite {
                function_name: function_name.to_string(),
                callee: callee.clone(),
                span: span.clone(),
                resolution: resolution.clone(),
            });
            facts.effect_events.extend(retain_events_for_call(
                function_name,
                callee,
                args,
                span,
                &resolution,
                hir,
                value_types,
            ));
            for arg in args {
                collect_body_facts_in_expr(hir, function_name, &arg.value, value_types, facts);
            }
        }
        Expr::Manage { value, span } => {
            if let Some((binding_name, value_span)) = direct_move_binding(value) {
                facts.effect_events.push(HirEffectEvent {
                    function_name: function_name.to_string(),
                    kind: HirEffectEventKind::Manage,
                    binding_name,
                    span: span.clone(),
                    value_span,
                });
            }
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Spawn { value, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Await { value, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            if let Some((binding_name, value_span)) = direct_ident(value) {
                facts.effect_events.push(HirEffectEvent {
                    function_name: function_name.to_string(),
                    kind: HirEffectEventKind::Take,
                    binding_name,
                    span: span.clone(),
                    value_span,
                });
            }
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Effect { value, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Try { value, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Field { base, name, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types);
            let base_type_display = base_type.as_ref().map(ToString::to_string);
            let resolved_base_type = base_type.clone();
            let resolved = resolved_base_type.as_ref().and_then(|ty| {
                let canonical = hir.canonical_type_name(&ty.to_string());
                let type_info = hir.type_info(&canonical)?;
                let field = type_info.fields.get(name)?;
                Some((type_info, ty, field))
            });
            facts.field_accesses.push(HirFieldAccess {
                function_name: function_name.to_string(),
                name: name.clone(),
                span: span.clone(),
                ty: resolved.map(|(type_info, ty, field)| {
                    substituted_field_type(hir, type_info, ty, field)
                }),
                is_handle: resolved.is_some_and(|(_, _, field)| field.is_handle || field.is_weak),
                is_weak: resolved.is_some_and(|(_, _, field)| field.is_weak),
                base_ty: base_type.clone(),
                base_type: base_type_display,
                type_name: resolved.map(|(type_info, ty, field)| {
                    substituted_field_type(hir, type_info, ty, field).to_string()
                }),
            });
            collect_body_facts_in_expr(hir, function_name, base, value_types, facts);
        }
        Expr::Index { base, index, .. } => {
            collect_body_facts_in_expr(hir, function_name, base, value_types, facts);
            collect_body_facts_in_expr(hir, function_name, index, value_types, facts);
        }
        Expr::Closure { body, .. } => {
            collect_body_facts_in_block(hir, function_name, body, value_types, facts);
        }
        Expr::Match { value, arms, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_body_facts_in_expr(hir, function_name, guard, value_types, facts);
                }
                collect_body_facts_in_block(hir, function_name, &arm.body, value_types, facts);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_body_facts_in_expr(hir, function_name, &field.value, value_types, facts);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_body_facts_in_expr(hir, function_name, &entry.key, value_types, facts);
                collect_body_facts_in_expr(hir, function_name, &entry.value, value_types, facts);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_body_facts_in_expr(hir, function_name, item, value_types, facts);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn direct_effect_retained_binding(
    expr: &Expr,
    hir: &Hir,
    value_types: &HirValueTypes,
) -> Option<(String, Span)> {
    match expr {
        Expr::Effect { value, .. } => retained_inline_binding(value, hir, value_types),
        _ => None,
    }
}

pub(super) fn retained_inline_binding(
    expr: &Expr,
    hir: &Hir,
    value_types: &HirValueTypes,
) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            retained_inline_binding(value, hir, value_types)
        }
        Expr::Field { base, name, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types)?;
            let field = hir.type_info(&base_type.to_string())?.fields.get(name)?;
            if field.is_handle || field.is_weak {
                return None;
            }
            let (binding_name, _) = retained_inline_binding(base, hir, value_types)?;
            Some((binding_name, span.clone()))
        }
        Expr::Call { callee, args, .. } if retained_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| retained_inline_binding(&arg.value, hir, value_types)),
        _ => None,
    }
}

pub(super) fn retained_wrapper_callee(callee: &Callee) -> bool {
    matches!(callee_name(callee), "Ok" | "Err" | "Some")
}

pub(super) fn direct_ident(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        _ => None,
    }
}

pub(super) fn direct_move_binding(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        Expr::Field { base, name, span } => {
            let (mut base_path, _) = direct_move_binding(base)?;
            base_path.push('.');
            base_path.push_str(name);
            Some((base_path, span.clone()))
        }
        _ => None,
    }
}
