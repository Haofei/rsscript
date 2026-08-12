//! Resource escape and managed closure capture indexing.

use super::*;

pub(super) fn index_managed_closure_uses_from_block(
    block: &HirBlock,
) -> HashMap<Span, Vec<(String, Span)>> {
    let mut closures = HashMap::new();
    collect_block_managed_closure_uses(block, &mut closures);
    closures
}

pub(super) fn index_resource_escapes_from_block(
    block: &HirBlock,
) -> HashMap<Span, Vec<ResourceEscape>> {
    let mut escapes = HashMap::new();
    collect_block_resource_escapes(block, &mut escapes);
    escapes
}

pub(super) fn collect_block_resource_escapes(
    block: &HirBlock,
    escapes_by_with_span: &mut HashMap<Span, Vec<ResourceEscape>>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::With {
                binding,
                body,
                span,
                ..
            } => {
                let mut escapes = Vec::new();
                collect_resource_escapes_in_block(binding, body, &mut escapes);
                escapes_by_with_span.insert(span.clone(), escapes);
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_expr_resource_escapes(value, escapes_by_with_span)
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_expr_resource_escapes(condition, escapes_by_with_span);
                collect_block_resource_escapes(then_body, escapes_by_with_span);
                if let Some(else_body) = else_body {
                    collect_block_resource_escapes(else_body, escapes_by_with_span);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_expr_resource_escapes(condition, escapes_by_with_span);
                }
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::For { iterable, body, .. } => {
                collect_expr_resource_escapes(iterable, escapes_by_with_span);
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_expr_resource_escapes(value, escapes_by_with_span);
                for arm in arms {
                    collect_block_resource_escapes(&arm.body, escapes_by_with_span);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_expr_resource_escapes(&arm.operation, escapes_by_with_span);
                    collect_block_resource_escapes(&arm.body, escapes_by_with_span);
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

pub(super) fn collect_expr_resource_escapes(
    expr: &HirExpr,
    escapes_by_with_span: &mut HashMap<Span, Vec<ResourceEscape>>,
) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_resource_escapes(&arg.value, escapes_by_with_span);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_resource_escapes(value, escapes_by_with_span);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_resource_escapes(left, escapes_by_with_span);
            collect_expr_resource_escapes(right, escapes_by_with_span);
        }
        HirExpr::Field { base, .. } => collect_expr_resource_escapes(base, escapes_by_with_span),
        HirExpr::Index { base, index, .. } => {
            collect_expr_resource_escapes(base, escapes_by_with_span);
            collect_expr_resource_escapes(index, escapes_by_with_span);
        }
        HirExpr::Closure { body, .. } => collect_block_resource_escapes(body, escapes_by_with_span),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_resource_escapes(value, escapes_by_with_span);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_resource_escapes(guard, escapes_by_with_span);
                }
                collect_block_resource_escapes(&arm.body, escapes_by_with_span);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_resource_escapes(&entry.key, escapes_by_with_span);
                collect_expr_resource_escapes(&entry.value, escapes_by_with_span);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_resource_escapes(&field.value, escapes_by_with_span);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_resource_escapes(item, escapes_by_with_span);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn collect_resource_escapes_in_block(
    binding: &str,
    block: &HirBlock,
    escapes: &mut Vec<ResourceEscape>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Return {
                value: Some(value), ..
            } if let Some(span) = resource_escape_operand_span(value, binding) => {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span);
            }
            HirStmt::Let {
                kind: HirBindingKind::ManagedLet,
                value: Some(value),
                ..
            } if let Some(span) = resource_escape_operand_span(value, binding) => {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span);
            }
            HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_resource_escapes_in_expr(binding, value, escapes)
            }
            HirStmt::With { body, .. } => collect_resource_escapes_in_block(binding, body, escapes),
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_resource_escapes_in_expr(binding, condition, escapes);
                collect_resource_escapes_in_block(binding, then_body, escapes);
                if let Some(else_body) = else_body {
                    collect_resource_escapes_in_block(binding, else_body, escapes);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_resource_escapes_in_expr(binding, condition, escapes);
                }
                collect_resource_escapes_in_block(binding, body, escapes);
            }
            HirStmt::For { iterable, body, .. } => {
                collect_resource_escapes_in_expr(binding, iterable, escapes);
                collect_resource_escapes_in_block(binding, body, escapes);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_resource_escapes_in_expr(binding, value, escapes);
                for arm in arms {
                    collect_resource_escapes_in_block(binding, &arm.body, escapes);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_resource_escapes_in_expr(binding, &arm.operation, escapes);
                    collect_resource_escapes_in_block(binding, &arm.body, escapes);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }

        if let HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(value),
            ..
        } = statement
            && let Some(span) = managed_binding_resource_capture_span(value, binding)
        {
            push_resource_escape(escapes, binding, ResourceEscapeKind::Capture, span);
        }
    }
}

pub(super) fn managed_binding_resource_capture_span(expr: &HirExpr, binding: &str) -> Option<Span> {
    match expr {
        HirExpr::Closure { body, span, .. } if hir_block_mentions_ident(body, binding) => {
            Some(span.clone())
        }
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => managed_binding_resource_capture_span(value, binding),
        HirExpr::Call { callee, args, .. } if resource_escape_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| managed_binding_resource_capture_span(&arg.value, binding)),
        _ => None,
    }
}

pub(super) fn collect_resource_escapes_in_expr(
    binding: &str,
    expr: &HirExpr,
    escapes: &mut Vec<ResourceEscape>,
) {
    match expr {
        HirExpr::Manage { value, span, .. } => {
            if resource_escape_operand_span(value, binding).is_some() {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span.clone());
            }
            collect_resource_escapes_in_expr(binding, value, escapes);
        }
        HirExpr::Call {
            callee,
            args,
            events,
            ..
        } if tempdir_keep_consumes_binding(callee, args, binding) => {
            for arg in args {
                if !take_ident_effect_expr(&arg.value, binding) {
                    collect_resource_escapes_in_expr(binding, &arg.value, escapes);
                }
            }
        }
        HirExpr::Call { args, events, .. } => {
            for event in events {
                if matches!(event.kind, HirEffectEventKind::Retain { .. })
                    && event.binding_name == binding
                {
                    push_resource_escape(
                        escapes,
                        binding,
                        ResourceEscapeKind::Escape,
                        event.value_span.clone(),
                    );
                }
            }
            for arg in args {
                collect_resource_escapes_in_expr(binding, &arg.value, escapes);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_resource_escapes_in_expr(binding, value, escapes);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_resource_escapes_in_expr(binding, left, escapes);
            collect_resource_escapes_in_expr(binding, right, escapes);
        }
        HirExpr::Field { base, .. } => collect_resource_escapes_in_expr(binding, base, escapes),
        HirExpr::Index { base, index, .. } => {
            collect_resource_escapes_in_expr(binding, base, escapes);
            collect_resource_escapes_in_expr(binding, index, escapes);
        }
        HirExpr::Closure { body, .. } => collect_resource_escapes_in_block(binding, body, escapes),
        HirExpr::Match { value, arms, .. } => {
            collect_resource_escapes_in_expr(binding, value, escapes);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_resource_escapes_in_expr(binding, guard, escapes);
                }
                collect_resource_escapes_in_block(binding, &arm.body, escapes);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_resource_escapes_in_expr(binding, &entry.key, escapes);
                collect_resource_escapes_in_expr(binding, &entry.value, escapes);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_resource_escapes_in_expr(binding, &field.value, escapes);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_resource_escapes_in_expr(binding, item, escapes);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn resource_escape_operand_span(expr: &HirExpr, binding: &str) -> Option<Span> {
    match expr {
        HirExpr::Ident { name, span, .. } if name == binding => Some(span.clone()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            resource_escape_operand_span(value, binding)
        }
        HirExpr::Call { callee, args, .. } if resource_escape_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| resource_escape_operand_span(&arg.value, binding)),
        _ => None,
    }
}

pub(super) fn tempdir_keep_consumes_binding(
    callee: &Callee,
    args: &[HirCallArg],
    binding: &str,
) -> bool {
    (matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "TempDir" && name == "keep"
    ) || matches!(
        callee,
        Callee::Name(name) if name == "TempDir.keep"
    )) && args.iter().any(|arg| {
        arg.name.as_deref().unwrap_or("dir") == "dir" && take_ident_effect_expr(&arg.value, binding)
    })
}

pub(super) fn take_ident_effect_expr(expr: &HirExpr, binding: &str) -> bool {
    matches!(
        expr,
        HirExpr::Effect {
            effect: ParamEffect::Take,
            value,
            ..
        } if matches!(
            value.as_ref(),
            HirExpr::Ident { name, .. } if name == binding
        )
    )
}

pub(super) fn resource_escape_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some")
    )
}

pub(super) fn hir_block_mentions_ident(block: &HirBlock, binding: &str) -> bool {
    rsscript_semantics::hir_block_identifier_uses(block)
        .iter()
        .any(|(name, _)| name == binding)
}

pub(super) fn push_resource_escape(
    escapes: &mut Vec<ResourceEscape>,
    binding: &str,
    kind: ResourceEscapeKind,
    span: Span,
) {
    let escape = ResourceEscape {
        binding: binding.to_string(),
        kind,
        span,
    };
    if !escapes.contains(&escape) {
        escapes.push(escape);
    }
}

pub(super) fn collect_block_managed_closure_uses(
    block: &HirBlock,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    for statement in &block.statements {
        collect_stmt_managed_closure_uses(statement, closures);
    }
}

pub(super) fn collect_stmt_managed_closure_uses(
    statement: &HirStmt,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    match statement {
        HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(HirExpr::Closure { body, .. }),
            span,
            ..
        } => {
            let mut uses = Vec::new();
            collect_hir_block_inline_capture_uses(body, &mut uses);
            closures.insert(span.clone(), uses);
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => collect_expr_managed_closure_uses(value, closures),
        HirStmt::With { resource, body, .. } => {
            collect_expr_managed_closure_uses(resource, closures);
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_managed_closure_uses(condition, closures);
            collect_block_managed_closure_uses(then_body, closures);
            if let Some(else_body) = else_body {
                collect_block_managed_closure_uses(else_body, closures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_managed_closure_uses(condition, closures);
            }
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_managed_closure_uses(iterable, closures);
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_managed_closure_uses(value, closures);
            for arm in arms {
                collect_block_managed_closure_uses(&arm.body, closures);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_managed_closure_uses(&arm.operation, closures);
                collect_block_managed_closure_uses(&arm.body, closures);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_expr_managed_closure_uses(
    expr: &HirExpr,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_managed_closure_uses(&arg.value, closures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_managed_closure_uses(value, closures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_managed_closure_uses(left, closures);
            collect_expr_managed_closure_uses(right, closures);
        }
        HirExpr::Field { base, .. } => collect_expr_managed_closure_uses(base, closures),
        HirExpr::Index { base, index, .. } => {
            collect_expr_managed_closure_uses(base, closures);
            collect_expr_managed_closure_uses(index, closures);
        }
        HirExpr::Closure { body, .. } => collect_block_managed_closure_uses(body, closures),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_managed_closure_uses(value, closures);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_managed_closure_uses(guard, closures);
                }
                collect_block_managed_closure_uses(&arm.body, closures);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_managed_closure_uses(&entry.key, closures);
                collect_expr_managed_closure_uses(&entry.value, closures);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_managed_closure_uses(&field.value, closures);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_managed_closure_uses(item, closures);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn collect_hir_stmt_effect_events(
    statement: &HirStmt,
    events: &mut Vec<HirEffectEvent>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_hir_expr_effect_events(value, events),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                collect_hir_expr_effect_events(read, events);
            }
            collect_hir_expr_effect_events(value, events);
        }
        HirStmt::With { resource, .. } => collect_hir_expr_effect_events(resource, events),
        HirStmt::If { condition, .. } => collect_hir_expr_effect_events(condition, events),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_hir_expr_effect_events(condition, events),
        HirStmt::For { iterable, .. } => collect_hir_expr_effect_events(iterable, events),
        HirStmt::Match {
            value,
            scrutinee_effect,
            ..
        } => {
            collect_hir_expr_effect_events(value, events);
            if *scrutinee_effect == Some(crate::syntax::ast::DataEffect::Take)
                && let Some((path, span)) = hir_expr_path(value)
            {
                events.push(HirEffectEvent {
                    function_name: String::new(),
                    kind: HirEffectEventKind::Take,
                    binding_name: path,
                    span: span.clone(),
                    value_span: span,
                });
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_hir_expr_effect_events(&arm.operation, events);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Loop {
            condition: None, ..
        }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_hir_expr_effect_events(expr: &HirExpr, events: &mut Vec<HirEffectEvent>) {
    match expr {
        HirExpr::Call {
            args,
            events: expr_events,
            ..
        } => {
            events.extend(expr_events.iter().cloned());
            for arg in args {
                collect_hir_expr_effect_events(&arg.value, events);
            }
        }
        HirExpr::Effect {
            value,
            events: expr_events,
            ..
        }
        | HirExpr::Manage {
            value,
            events: expr_events,
            ..
        } => {
            events.extend(expr_events.iter().cloned());
            collect_hir_expr_effect_events(value, events);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            collect_hir_expr_effect_events(value, events)
        }
        HirExpr::Try { value, .. } => collect_hir_expr_effect_events(value, events),
        HirExpr::Binary { left, right, .. } => {
            collect_hir_expr_effect_events(left, events);
            collect_hir_expr_effect_events(right, events);
        }
        HirExpr::Field { base, .. } => collect_hir_expr_effect_events(base, events),
        HirExpr::Index { base, index, .. } => {
            collect_hir_expr_effect_events(base, events);
            collect_hir_expr_effect_events(index, events);
        }
        HirExpr::Match {
            value,
            scrutinee_effect,
            arms,
            ..
        } => {
            collect_hir_expr_effect_events(value, events);
            if *scrutinee_effect == Some(crate::syntax::ast::DataEffect::Take)
                && let Some((path, span)) = hir_expr_path(value)
            {
                events.push(HirEffectEvent {
                    function_name: String::new(),
                    kind: HirEffectEventKind::Take,
                    binding_name: path,
                    span: span.clone(),
                    value_span: span,
                });
            }
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_hir_expr_effect_events(guard, events);
                }
                for statement in &arm.body.statements {
                    collect_hir_stmt_effect_events(statement, events);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_hir_expr_effect_events(&entry.key, events);
                collect_hir_expr_effect_events(&entry.value, events);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_hir_expr_effect_events(&field.value, events);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_hir_expr_effect_events(item, events);
            }
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn collect_hir_block_inline_capture_uses(
    block: &HirBlock,
    uses: &mut Vec<(String, Span)>,
) {
    for statement in &block.statements {
        collect_hir_stmt_inline_capture_uses(statement, uses);
        match statement {
            HirStmt::With { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_hir_block_inline_capture_uses(then_body, uses);
                if let Some(else_body) = else_body {
                    collect_hir_block_inline_capture_uses(else_body, uses);
                }
            }
            HirStmt::Loop { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
            HirStmt::For { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_hir_block_inline_capture_uses(&arm.body, uses);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_hir_block_inline_capture_uses(&arm.body, uses);
                }
            }
            HirStmt::Let { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Assign { .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn collect_hir_stmt_inline_capture_uses(
    statement: &HirStmt,
    uses: &mut Vec<(String, Span)>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_hir_expr_inline_capture_uses(value, uses),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                collect_hir_expr_inline_capture_uses(read, uses);
            }
            collect_hir_expr_inline_capture_uses(value, uses);
        }
        HirStmt::With { resource, .. } => collect_hir_expr_inline_capture_uses(resource, uses),
        HirStmt::If { condition, .. } => collect_hir_expr_inline_capture_uses(condition, uses),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_hir_expr_inline_capture_uses(condition, uses),
        HirStmt::For { iterable, .. } => collect_hir_expr_inline_capture_uses(iterable, uses),
        HirStmt::Match { value, .. } => collect_hir_expr_inline_capture_uses(value, uses),
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_hir_expr_inline_capture_uses(&arm.operation, uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Loop {
            condition: None, ..
        }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_hir_expr_inline_capture_uses(expr: &HirExpr, uses: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => uses.push((name.clone(), span.clone())),
        HirExpr::Field { base, access, .. } => {
            if !access.is_handle {
                collect_hir_expr_inline_capture_uses(base, uses);
            }
        }
        HirExpr::Index { base, index, .. } => {
            collect_hir_expr_inline_capture_uses(base, uses);
            collect_hir_expr_inline_capture_uses(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_hir_expr_inline_capture_uses(&arg.value, uses);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_hir_expr_inline_capture_uses(&field.value, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_hir_expr_inline_capture_uses(&entry.key, uses);
                collect_hir_expr_inline_capture_uses(&entry.value, uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_hir_expr_inline_capture_uses(item, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_hir_expr_inline_capture_uses(value, uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_hir_expr_inline_capture_uses(left, uses);
            collect_hir_expr_inline_capture_uses(right, uses);
        }
        HirExpr::Closure { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Match { .. }
        | HirExpr::Unknown(_) => {}
    }
}
