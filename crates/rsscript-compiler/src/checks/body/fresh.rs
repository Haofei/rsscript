use super::*;

pub(super) fn expr_is_fresh_shell(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { resolution, .. } => match resolution {
            CallResolution::Resolved {
                signature,
                kind:
                    ResolvedCalleeKind::Constructor {
                        type_kind: HirTypeKind::Struct,
                    },
            } => signature.returns_fresh,
            CallResolution::Resolved { signature, .. } => signature.returns_fresh,
            CallResolution::EnumVariant
            | CallResolution::Ambiguous { .. }
            | CallResolution::Unknown => false,
        },
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => expr_is_fresh_shell(value),
        HirExpr::Ident { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Match { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => false,
    }
}

pub(super) fn check_read_view_not_exclusive(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) -> bool {
    let Some(path) = place_path(value) else {
        return false;
    };
    if !state.is_read_view(&path.base) {
        return false;
    }
    read_view_mutation_diagnostic(analyzer, &path.base, span.clone());
    true
}

pub(super) fn hir_expr_hint(expr: &HirExpr) -> String {
    match expr {
        HirExpr::Call { callee, .. } => body_callee_display(callee),
        HirExpr::Try { value, .. } => format!("{}?", hir_expr_hint(value)),
        HirExpr::Ident { name, .. } => name.clone(),
        _ => "fresh_expr".to_string(),
    }
}

pub(super) fn body_callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            (*effect).unwrap_or(DataEffect::Read).as_str(),
            body_expr_label(receiver)
        ),
    }
}

pub(super) fn body_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::CharLiteral(value, _) | Expr::MultilineString(value, _) => {
            format!("{value:?}")
        }
        Expr::Field { base, name, .. } => format!("{}.{}", body_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", body_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", body_callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            body_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

pub(super) fn check_constructor_field_initializers(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    expr: &HirExpr,
    state: &BodyState,
) {
    let HirExpr::Call { resolution, .. } = expr else {
        return;
    };
    let CallResolution::Resolved {
        signature,
        kind:
            ResolvedCalleeKind::Constructor {
                type_kind: HirTypeKind::Struct | HirTypeKind::Class,
            },
    } = resolution
    else {
        return;
    };
    let Some(type_info) = analyzer.hir.type_info(&signature.name) else {
        return;
    };
    let fields = type_info.fields.clone();
    let constructor_name = body_callee_display(callee);

    for arg in args {
        let Some(name) = constructor_field_arg_name(arg, &fields) else {
            continue;
        };
        let Some(field) = fields.get(name) else {
            continue;
        };
        let actual_effect = expr_data_effect(&arg.value);
        if field.is_weak && !is_weak_handle_producing_expr(&arg.value) {
            analyzer.diagnostics.push(
                rsscript_semantics::weak_field_requires_weak_handle_diagnostic(
                    &constructor_name,
                    name,
                    hir_expr_span(&arg.value).clone(),
                ),
            );
        } else if field.is_handle && actual_effect != Some("read") {
            analyzer
                .diagnostics
                .push(rsscript_semantics::constructor_field_effect_diagnostic(
                    &constructor_name,
                    name,
                    "read",
                    hir_expr_span(&arg.value),
                    "Handle fields store managed handles and must be initialized from an explicit `read` value.",
                ));
        } else if !field.is_handle
            && constructor_arg_uses_local_inline_place(&arg.value, state)
            && actual_effect != Some("take")
        {
            analyzer
                .diagnostics
                .push(rsscript_semantics::constructor_field_effect_diagnostic(
                    &constructor_name,
                    name,
                    "take",
                    hir_expr_span(&arg.value),
                    "Inline fields take ownership of non-Copy local values stored inside the constructed struct.",
                ));
        } else if !field.is_handle
            && field
                .ty
                .root_name()
                .and_then(|name| analyzer.hir.type_kind(name))
                .is_some()
            && !is_copy_type_name(&field.ty.to_string())
            && constructor_arg_uses_managed_inline_value(analyzer, &arg.value, state)
        {
            analyzer.diagnostics.push(
                rsscript_semantics::managed_inline_constructor_field_diagnostic(
                    &constructor_name,
                    name,
                    hir_expr_span(&arg.value).clone(),
                ),
            );
        }
    }
}

pub(super) fn constructor_field_arg_name<'a>(
    arg: &'a HirCallArg,
    fields: &HashMap<String, FieldInfo>,
) -> Option<&'a str> {
    if let Some(name) = arg.name.as_deref() {
        return Some(name);
    }
    let HirExpr::Ident { name, .. } = &arg.value else {
        return None;
    };
    fields.contains_key(name).then_some(name.as_str())
}

pub(super) fn constructor_arg_uses_local_inline_place(expr: &HirExpr, state: &BodyState) -> bool {
    let Some(path) = constructor_arg_place_path(expr) else {
        return false;
    };
    state.is_local(&path.base) && !path.crosses_handle
}

pub(super) fn constructor_arg_place_path(expr: &HirExpr) -> Option<PlacePath> {
    match expr {
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            constructor_arg_place_path(value)
        }
        HirExpr::Ident { .. } | HirExpr::Field { .. } | HirExpr::Index { .. } => place_path(expr),
        HirExpr::Manage { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Match { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn constructor_arg_uses_managed_inline_value(
    analyzer: &Analyzer<'_>,
    expr: &HirExpr,
    state: &BodyState,
) -> bool {
    match expr {
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            constructor_arg_uses_managed_inline_value(analyzer, value, state)
        }
        HirExpr::Manage { .. } => true,
        HirExpr::Ident { .. } | HirExpr::Field { .. } | HirExpr::Index { .. } => {
            constructor_arg_place_path(expr)
                .is_some_and(|path| path.crosses_handle || state.is_managed(&path.base))
        }
        HirExpr::Call { resolution, .. } => match resolution {
            CallResolution::Resolved {
                signature,
                kind:
                    ResolvedCalleeKind::Constructor {
                        type_kind: HirTypeKind::Struct,
                    },
            } if signature.returns_fresh => false,
            CallResolution::Resolved {
                signature,
                kind:
                    ResolvedCalleeKind::Constructor {
                        type_kind: HirTypeKind::Class,
                    },
            } if signature.returns_fresh => true,
            CallResolution::Resolved { signature, .. } => {
                signature.return_ty.as_ref().is_some_and(|return_ty| {
                    let type_name = return_ty.root_name().unwrap_or_default();
                    !signature.returns_fresh
                        && !is_copy_type_name(type_name)
                        && analyzer.hir.type_kind(type_name).is_some()
                })
            }
            CallResolution::EnumVariant
            | CallResolution::Ambiguous { .. }
            | CallResolution::Unknown => false,
        },
        HirExpr::ObjectLiteral { fields, .. } => fields
            .iter()
            .any(|field| constructor_arg_uses_managed_inline_value(analyzer, &field.value, state)),
        HirExpr::MapLiteral { entries, .. } => entries.iter().any(|entry| {
            constructor_arg_uses_managed_inline_value(analyzer, &entry.key, state)
                || constructor_arg_uses_managed_inline_value(analyzer, &entry.value, state)
        }),
        HirExpr::ArrayLiteral { items, .. } => items
            .iter()
            .any(|item| constructor_arg_uses_managed_inline_value(analyzer, item, state)),
        HirExpr::Binary { .. }
        | HirExpr::Match { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Unknown(_) => false,
    }
}

pub(super) fn is_weak_handle_producing_expr(expr: &HirExpr) -> bool {
    let HirExpr::Call { callee, args, .. } = expr else {
        return false;
    };
    if !matches!(
        callee,
        Callee::Qualified { namespace, name }
            if namespace == "Weak" && matches!(name.as_str(), "from" | "downgrade")
    ) {
        return false;
    }
    matches!(
        args.as_slice(),
        [HirCallArg {
            name: Some(name),
            value:
                HirExpr::Effect {
                    effect: ParamEffect::Read,
                    ..
                },
            ..
        }] if name == "value"
    )
}

pub(super) fn expr_data_effect(expr: &HirExpr) -> Option<&'static str> {
    match expr {
        HirExpr::Effect { effect, .. } => Some(effect.as_str()),
        _ => None,
    }
}

pub(super) fn check_spawn_captures(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    state: &BodyState,
) {
    let mut captures = Vec::new();
    collect_spawn_capture_idents(value, &mut captures);
    for (name, span) in captures {
        if state.is_local(&name) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::spawn_local_capture_diagnostic(
                    &name, span,
                ));
        } else if state.is_resource(&name) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::resource_escape_diagnostic(&name, span));
        }
    }
}

pub(super) fn collect_spawn_capture_idents(expr: &HirExpr, captures: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => captures.push((name.clone(), span.clone())),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            collect_spawn_capture_idents(value, captures);
        }
        HirExpr::Manage { .. } => {}
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_spawn_capture_idents(&arg.value, captures);
            }
        }
        HirExpr::Field { base, .. } => collect_spawn_capture_idents(base, captures),
        HirExpr::Index { base, index, .. } => {
            collect_spawn_capture_idents(base, captures);
            collect_spawn_capture_idents(index, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_spawn_capture_idents(left, captures);
            collect_spawn_capture_idents(right, captures);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            collect_spawn_capture_idents(value, captures);
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                collect_spawn_capture_idents_from_stmt(statement, captures);
            }
        }
        HirExpr::Match { value, arms, .. } => {
            collect_spawn_capture_idents(value, captures);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_spawn_capture_idents(guard, captures);
                }
                for statement in &arm.body.statements {
                    collect_spawn_capture_idents_from_stmt(statement, captures);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_spawn_capture_idents(&entry.key, captures);
                collect_spawn_capture_idents(&entry.value, captures);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_spawn_capture_idents(&field.value, captures);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_spawn_capture_idents(item, captures);
            }
        }
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn collect_spawn_capture_idents_from_stmt(
    statement: &HirStmt,
    captures: &mut Vec<(String, Span)>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_spawn_capture_idents(value, captures),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                collect_spawn_capture_idents(read, captures);
            }
            collect_spawn_capture_idents(value, captures);
        }
        HirStmt::With { resource, body, .. } => {
            collect_spawn_capture_idents(resource, captures);
            for statement in &body.statements {
                collect_spawn_capture_idents_from_stmt(statement, captures);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_spawn_capture_idents(condition, captures);
            for statement in &then_body.statements {
                collect_spawn_capture_idents_from_stmt(statement, captures);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    collect_spawn_capture_idents_from_stmt(statement, captures);
                }
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_spawn_capture_idents(condition, captures);
            }
            for statement in &body.statements {
                collect_spawn_capture_idents_from_stmt(statement, captures);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            collect_spawn_capture_idents(iterable, captures);
            for statement in &body.statements {
                collect_spawn_capture_idents_from_stmt(statement, captures);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            collect_spawn_capture_idents(value, captures);
            for arm in arms {
                for statement in &arm.body.statements {
                    collect_spawn_capture_idents_from_stmt(statement, captures);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_spawn_capture_idents(&arm.operation, captures);
                for statement in &arm.body.statements {
                    collect_spawn_capture_idents_from_stmt(statement, captures);
                }
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}
