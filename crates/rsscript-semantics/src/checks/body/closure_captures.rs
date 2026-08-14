use super::*;

pub(super) fn check_explicit_closure_captures_block(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    binding_names: &HashSet<String>,
) {
    for statement in &block.statements {
        check_explicit_closure_captures_stmt(analyzer, statement, binding_names);
    }
}

pub(super) fn check_explicit_closure_captures_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    binding_names: &HashSet<String>,
) {
    match statement {
        HirStmt::Let { value, .. } => {
            if let Some(value) = value {
                check_explicit_closure_captures_expr(analyzer, value, binding_names);
            }
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_explicit_closure_captures_expr(analyzer, value, binding_names);
            }
        }
        HirStmt::With { resource, body, .. } => {
            check_explicit_closure_captures_expr(analyzer, resource, binding_names);
            check_explicit_closure_captures_block(analyzer, body, binding_names);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_explicit_closure_captures_expr(analyzer, condition, binding_names);
            check_explicit_closure_captures_block(analyzer, then_body, binding_names);
            if let Some(else_body) = else_body {
                check_explicit_closure_captures_block(analyzer, else_body, binding_names);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_explicit_closure_captures_expr(analyzer, condition, binding_names);
            }
            check_explicit_closure_captures_block(analyzer, body, binding_names);
        }
        HirStmt::For { iterable, body, .. } => {
            check_explicit_closure_captures_expr(analyzer, iterable, binding_names);
            check_explicit_closure_captures_block(analyzer, body, binding_names);
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_explicit_closure_captures_expr(analyzer, &arm.operation, binding_names);
                check_explicit_closure_captures_block(analyzer, &arm.body, binding_names);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_explicit_closure_captures_expr(analyzer, value, binding_names);
            for arm in arms {
                check_explicit_closure_captures_block(analyzer, &arm.body, binding_names);
            }
        }
        HirStmt::Expr(expr) => check_explicit_closure_captures_expr(analyzer, expr, binding_names),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                check_explicit_closure_captures_expr(analyzer, read, binding_names);
            }
            check_explicit_closure_captures_expr(analyzer, value, binding_names);
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_explicit_closure_captures_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    binding_names: &HashSet<String>,
) {
    match expr {
        HirExpr::Closure {
            params,
            captures,
            body,
            explicit,
            ..
        } => {
            if *explicit {
                check_one_explicit_closure_capture_contract(
                    analyzer,
                    params,
                    captures,
                    body,
                    binding_names,
                );
            }
            check_explicit_closure_captures_block(analyzer, body, binding_names);
        }
        HirExpr::Binary { left, right, .. } => {
            check_explicit_closure_captures_expr(analyzer, left, binding_names);
            check_explicit_closure_captures_expr(analyzer, right, binding_names);
        }
        HirExpr::Field { base, .. } => {
            check_explicit_closure_captures_expr(analyzer, base, binding_names);
        }
        HirExpr::Index { base, index, .. } => {
            check_explicit_closure_captures_expr(analyzer, base, binding_names);
            check_explicit_closure_captures_expr(analyzer, index, binding_names);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_explicit_closure_captures_expr(analyzer, &arg.value, binding_names);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_explicit_closure_captures_expr(analyzer, value, binding_names);
        }
        HirExpr::Match { value, arms, .. } => {
            check_explicit_closure_captures_expr(analyzer, value, binding_names);
            for arm in arms {
                check_explicit_closure_captures_block(analyzer, &arm.body, binding_names);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_explicit_closure_captures_expr(analyzer, &entry.key, binding_names);
                check_explicit_closure_captures_expr(analyzer, &entry.value, binding_names);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                check_explicit_closure_captures_expr(analyzer, &field.value, binding_names);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                check_explicit_closure_captures_expr(analyzer, item, binding_names);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn check_one_explicit_closure_capture_contract(
    analyzer: &mut Analyzer<'_>,
    params: &[String],
    captures: &[crate::hir::HirClosureCapture],
    body: &HirBlock,
    binding_names: &HashSet<String>,
) {
    let param_names = params.iter().cloned().collect::<HashSet<_>>();
    let declared = captures
        .iter()
        .map(|capture| (capture.name.clone(), (capture.effect, capture.span.clone())))
        .collect::<HashMap<_, _>>();
    let actual = explicit_closure_actual_captures(body, &param_names, binding_names);

    for (name, effect) in &actual {
        match declared.get(name) {
            // The declared capture must GRANT at least the access the body uses,
            // but it may grant more. In particular a `take` (move/ownership)
            // capture — required to soundly store a non-`Copy` value in an
            // escaping `owned Fn` — backs a body that only `read`s the value (a
            // repeatedly-called stored closure owns its capture and reads it
            // each call). Only flag when the body needs STRONGER access than the
            // declared capture provides.
            Some((declared_effect, span))
                if capture_effect_rank(*effect) > capture_effect_rank(*declared_effect) =>
            {
                analyzer.diagnostics.push(
                    rsscript_semantics::explicit_closure_capture_contract_diagnostic(
                        name,
                        declared_effect.as_str(),
                        effect.as_str(),
                        span.clone(),
                    ),
                );
            }
            Some(_) => {}
            None => {
                analyzer.diagnostics.push(
                    rsscript_semantics::explicit_closure_missing_capture_diagnostic(
                        name,
                        effect.as_str(),
                        body.span.clone(),
                    ),
                );
            }
        }
    }
    for (name, (_, span)) in declared {
        if !actual.contains_key(&name) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::explicit_closure_unused_capture_diagnostic(&name, span));
        }
    }
}

pub(super) fn explicit_closure_actual_captures(
    body: &HirBlock,
    params: &HashSet<String>,
    binding_names: &HashSet<String>,
) -> HashMap<String, ParamEffect> {
    let mut uses = HashSet::new();
    collect_block_uses(body, &mut uses);
    for param in params {
        uses.remove(param);
    }
    // A name the closure itself binds (`let rhs = ...` inside the body) is a
    // closure-local, not a capture — even though the flat function-wide
    // `binding_names` set contains it. Subtract the closure's own bindings so a
    // body-local does not get mistaken for a missing/extra capture.
    let mut closure_local_bindings = HashSet::new();
    collect_hir_block_bindings(body, &mut closure_local_bindings);
    for name in &closure_local_bindings {
        uses.remove(name);
    }
    let mut actual = uses
        .into_iter()
        .filter(|name| binding_names.contains(name))
        .map(|name| (name, ParamEffect::Read))
        .collect::<HashMap<_, _>>();

    let mut bound = params.clone();
    bound.extend(closure_local_bindings.iter().cloned());
    let mut accesses = Vec::new();
    collect_closure_effect_accesses_block(body, &bound, &mut accesses);
    for access in accesses {
        if binding_names.contains(&access.path.base)
            && !closure_local_bindings.contains(&access.path.base)
        {
            let current = actual
                .get(&access.path.base)
                .copied()
                .unwrap_or(ParamEffect::Read);
            let next = strongest_capture_effect(current, access.effect);
            actual.insert(access.path.base, next);
        }
    }
    actual
}

/// Names bound by statements directly within `block` (and its nested control-
/// flow blocks): `let`/`local` names, `with`/`for` loop bindings, and match-arm
/// pattern bindings. Used to exclude a closure's own body-local bindings from
/// its capture set. Nested closure expressions introduce their own scope and
/// are intentionally not descended into here.
fn collect_hir_block_bindings(block: &HirBlock, names: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let { name, .. } => {
                names.insert(name.clone());
            }
            HirStmt::With { binding, body, .. } => {
                names.insert(binding.clone());
                collect_hir_block_bindings(body, names);
            }
            HirStmt::For { binding, body, .. } => {
                names.insert(binding.clone());
                collect_hir_block_bindings(body, names);
            }
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_hir_block_bindings(then_body, names);
                if let Some(else_body) = else_body {
                    collect_hir_block_bindings(else_body, names);
                }
            }
            HirStmt::Loop { body, .. } => collect_hir_block_bindings(body, names),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    for binding in arm.pattern.binding_names() {
                        names.insert(binding.to_string());
                    }
                    collect_hir_block_bindings(&arm.body, names);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_hir_block_bindings(&arm.body, names);
                }
            }
            HirStmt::Assign { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

/// Access-strength rank: `read` < `mut` < `take`. A declared capture is valid
/// when its rank is >= the body's actual-usage rank (it grants enough access).
fn capture_effect_rank(effect: ParamEffect) -> u8 {
    match effect {
        ParamEffect::Read => 0,
        ParamEffect::Mut => 1,
        ParamEffect::Take => 2,
    }
}

/// `read` < `mut` < `take` for the syntax-level `DataEffect` capture check.
fn data_effect_rank(effect: crate::syntax::ast::DataEffect) -> u8 {
    match effect {
        crate::syntax::ast::DataEffect::Read => 0,
        crate::syntax::ast::DataEffect::Mut => 1,
        crate::syntax::ast::DataEffect::Take => 2,
    }
}

pub(super) fn strongest_capture_effect(left: ParamEffect, right: ParamEffect) -> ParamEffect {
    match (left, right) {
        (ParamEffect::Take, _) | (_, ParamEffect::Take) => ParamEffect::Take,
        (ParamEffect::Mut, _) | (_, ParamEffect::Mut) => ParamEffect::Mut,
        _ => ParamEffect::Read,
    }
}

pub(super) fn check_local_class_bindings(
    analyzer: &mut Analyzer<'_>,
    body: &crate::hir::HirFunctionBody,
) {
    for binding in &body.bindings {
        if binding.kind == HirBindingKind::LocalLet
            && binding.ty.as_ref().is_some_and(|ty| {
                analyzer.hir.type_kind(&ty.to_string()) == Some(HirTypeKind::Class)
            })
        {
            analyzer
                .diagnostics
                .push(rsscript_semantics::local_class_binding_diagnostic(
                    &binding.name,
                    binding.span.clone(),
                ));
        }
    }
}

pub(super) fn check_explicit_closure_capture_effects_syntax(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
) {
    let mut binding_names = function
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<HashSet<_>>();
    collect_syntax_block_bindings(&function.body, &mut binding_names);
    check_explicit_closure_capture_effects_syntax_block(analyzer, &function.body, &binding_names);
}

pub(super) fn collect_syntax_block_bindings(block: &SyntaxBlock, names: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement {
            SyntaxStmt::Let(stmt) => {
                names.insert(stmt.name.clone());
                if let Some(value) = &stmt.value {
                    collect_syntax_expr_bindings(value, names);
                }
            }
            SyntaxStmt::LetElse(stmt) => {
                names.insert(stmt.binding_name.clone());
                collect_syntax_expr_bindings(&stmt.value, names);
                collect_syntax_block_bindings(&stmt.else_body, names);
            }
            SyntaxStmt::With(stmt) => {
                names.insert(stmt.binding.clone());
                collect_syntax_expr_bindings(&stmt.resource, names);
                collect_syntax_block_bindings(&stmt.body, names);
            }
            SyntaxStmt::If(stmt) => {
                collect_syntax_expr_bindings(&stmt.condition, names);
                collect_syntax_block_bindings(&stmt.then_body, names);
                if let Some(else_body) = &stmt.else_body {
                    collect_syntax_block_bindings(else_body, names);
                }
            }
            SyntaxStmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_syntax_expr_bindings(condition, names);
                }
                collect_syntax_block_bindings(&stmt.body, names);
            }
            SyntaxStmt::For(stmt) => {
                names.insert(stmt.binding.clone());
                collect_syntax_expr_bindings(&stmt.iterable, names);
                collect_syntax_block_bindings(&stmt.body, names);
            }
            SyntaxStmt::Match(stmt) => {
                collect_syntax_expr_bindings(&stmt.value, names);
                for arm in &stmt.arms {
                    collect_syntax_block_bindings(&arm.body, names);
                }
            }
            SyntaxStmt::TaskGroup(stmt) => collect_syntax_block_bindings(&stmt.body, names),
            SyntaxStmt::Select(stmt) => {
                for arm in &stmt.arms {
                    if arm.binding != "_" {
                        names.insert(arm.binding.clone());
                    }
                    collect_syntax_expr_bindings(&arm.operation, names);
                    collect_syntax_block_bindings(&arm.body, names);
                }
            }
            SyntaxStmt::Assign(stmt) => {
                collect_syntax_expr_bindings(&stmt.target, names);
                collect_syntax_expr_bindings(&stmt.value, names);
            }
            SyntaxStmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_syntax_expr_bindings(value, names);
                }
            }
            SyntaxStmt::Expr(expr) => collect_syntax_expr_bindings(expr, names),
            SyntaxStmt::MalformedWith(_)
            | SyntaxStmt::MalformedIf(_)
            | SyntaxStmt::MalformedLoop(_)
            | SyntaxStmt::MalformedFor(_)
            | SyntaxStmt::MalformedMatch(_)
            | SyntaxStmt::Break(_)
            | SyntaxStmt::Continue(_)
            | SyntaxStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn collect_syntax_expr_bindings(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::Closure { body, .. } => collect_syntax_block_bindings(body, names),
        Expr::Binary { left, right, .. } => {
            collect_syntax_expr_bindings(left, names);
            collect_syntax_expr_bindings(right, names);
        }
        Expr::Field { base, .. } => collect_syntax_expr_bindings(base, names),
        Expr::Index { base, index, .. } => {
            collect_syntax_expr_bindings(base, names);
            collect_syntax_expr_bindings(index, names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_syntax_expr_bindings(&arg.value, names);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => collect_syntax_expr_bindings(value, names),
        Expr::Match { value, arms, .. } => {
            collect_syntax_expr_bindings(value, names);
            for arm in arms {
                collect_syntax_block_bindings(&arm.body, names);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_syntax_expr_bindings(&entry.key, names);
                collect_syntax_expr_bindings(&entry.value, names);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_syntax_expr_bindings(&field.value, names);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_syntax_expr_bindings(item, names);
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

pub(super) fn check_explicit_closure_capture_effects_syntax_block(
    analyzer: &mut Analyzer<'_>,
    block: &SyntaxBlock,
    binding_names: &HashSet<String>,
) {
    for statement in &block.statements {
        check_explicit_closure_capture_effects_syntax_stmt(analyzer, statement, binding_names);
    }
}

pub(super) fn check_explicit_closure_capture_effects_syntax_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &SyntaxStmt,
    binding_names: &HashSet<String>,
) {
    match statement {
        SyntaxStmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                check_explicit_closure_capture_effects_syntax_expr(analyzer, value, binding_names);
            }
        }
        SyntaxStmt::LetElse(stmt) => {
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.value,
                binding_names,
            );
            check_explicit_closure_capture_effects_syntax_block(
                analyzer,
                &stmt.else_body,
                binding_names,
            );
        }
        SyntaxStmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                check_explicit_closure_capture_effects_syntax_expr(analyzer, value, binding_names);
            }
        }
        SyntaxStmt::With(stmt) => {
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.resource,
                binding_names,
            );
            check_explicit_closure_capture_effects_syntax_block(
                analyzer,
                &stmt.body,
                binding_names,
            );
        }
        SyntaxStmt::If(stmt) => {
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.condition,
                binding_names,
            );
            check_explicit_closure_capture_effects_syntax_block(
                analyzer,
                &stmt.then_body,
                binding_names,
            );
            if let Some(else_body) = &stmt.else_body {
                check_explicit_closure_capture_effects_syntax_block(
                    analyzer,
                    else_body,
                    binding_names,
                );
            }
        }
        SyntaxStmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                check_explicit_closure_capture_effects_syntax_expr(
                    analyzer,
                    condition,
                    binding_names,
                );
            }
            check_explicit_closure_capture_effects_syntax_block(
                analyzer,
                &stmt.body,
                binding_names,
            );
        }
        SyntaxStmt::For(stmt) => {
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.iterable,
                binding_names,
            );
            check_explicit_closure_capture_effects_syntax_block(
                analyzer,
                &stmt.body,
                binding_names,
            );
        }
        SyntaxStmt::Match(stmt) => {
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.value,
                binding_names,
            );
            for arm in &stmt.arms {
                check_explicit_closure_capture_effects_syntax_block(
                    analyzer,
                    &arm.body,
                    binding_names,
                );
            }
        }
        SyntaxStmt::TaskGroup(stmt) => {
            check_explicit_closure_capture_effects_syntax_block(
                analyzer,
                &stmt.body,
                binding_names,
            );
        }
        SyntaxStmt::Select(stmt) => {
            for arm in &stmt.arms {
                check_explicit_closure_capture_effects_syntax_expr(
                    analyzer,
                    &arm.operation,
                    binding_names,
                );
                check_explicit_closure_capture_effects_syntax_block(
                    analyzer,
                    &arm.body,
                    binding_names,
                );
            }
        }
        SyntaxStmt::Assign(stmt) => {
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.target,
                binding_names,
            );
            check_explicit_closure_capture_effects_syntax_expr(
                analyzer,
                &stmt.value,
                binding_names,
            );
        }
        SyntaxStmt::Expr(expr) => {
            check_explicit_closure_capture_effects_syntax_expr(analyzer, expr, binding_names);
        }
        SyntaxStmt::MalformedWith(_)
        | SyntaxStmt::MalformedIf(_)
        | SyntaxStmt::MalformedLoop(_)
        | SyntaxStmt::MalformedFor(_)
        | SyntaxStmt::MalformedMatch(_)
        | SyntaxStmt::Break(_)
        | SyntaxStmt::Continue(_)
        | SyntaxStmt::Unknown(_) => {}
    }
}

pub(super) fn check_explicit_closure_capture_effects_syntax_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &Expr,
    binding_names: &HashSet<String>,
) {
    match expr {
        Expr::Closure {
            captures,
            explicit,
            body,
            ..
        } => {
            if *explicit {
                let actual = syntax_closure_actual_capture_effects(body, binding_names);
                for capture in captures {
                    // The declared capture must GRANT at least the access the
                    // body uses, but may grant more — a `take` (move/ownership)
                    // capture soundly backs a `read`-only body (the canonical
                    // way to store a non-`Copy` value in an escaping `owned Fn`
                    // that is called repeatedly). Only flag when the body needs
                    // STRONGER access than the declared capture provides.
                    if let Some(actual_effect) = actual.get(&capture.name)
                        && data_effect_rank(*actual_effect) > data_effect_rank(capture.effect)
                    {
                        analyzer.diagnostics.push(
                            rsscript_semantics::explicit_closure_capture_contract_diagnostic(
                                &capture.name,
                                param_effect_from_data_effect_syntax(capture.effect).as_str(),
                                param_effect_from_data_effect_syntax(*actual_effect).as_str(),
                                capture.span.clone(),
                            ),
                        );
                    }
                }
            }
            check_explicit_closure_capture_effects_syntax_block(analyzer, body, binding_names);
        }
        Expr::Binary { left, right, .. } => {
            check_explicit_closure_capture_effects_syntax_expr(analyzer, left, binding_names);
            check_explicit_closure_capture_effects_syntax_expr(analyzer, right, binding_names);
        }
        Expr::Field { base, .. } => {
            check_explicit_closure_capture_effects_syntax_expr(analyzer, base, binding_names);
        }
        Expr::Index { base, index, .. } => {
            check_explicit_closure_capture_effects_syntax_expr(analyzer, base, binding_names);
            check_explicit_closure_capture_effects_syntax_expr(analyzer, index, binding_names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                check_explicit_closure_capture_effects_syntax_expr(
                    analyzer,
                    &arg.value,
                    binding_names,
                );
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            check_explicit_closure_capture_effects_syntax_expr(analyzer, value, binding_names);
        }
        Expr::Match { value, arms, .. } => {
            check_explicit_closure_capture_effects_syntax_expr(analyzer, value, binding_names);
            for arm in arms {
                check_explicit_closure_capture_effects_syntax_block(
                    analyzer,
                    &arm.body,
                    binding_names,
                );
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_explicit_closure_capture_effects_syntax_expr(
                    analyzer,
                    &entry.key,
                    binding_names,
                );
                check_explicit_closure_capture_effects_syntax_expr(
                    analyzer,
                    &entry.value,
                    binding_names,
                );
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                check_explicit_closure_capture_effects_syntax_expr(
                    analyzer,
                    &field.value,
                    binding_names,
                );
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                check_explicit_closure_capture_effects_syntax_expr(analyzer, item, binding_names);
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

pub(super) fn syntax_closure_actual_capture_effects(
    body: &SyntaxBlock,
    binding_names: &HashSet<String>,
) -> HashMap<String, DataEffect> {
    let mut actual = HashMap::new();
    collect_syntax_capture_effects_block(body, binding_names, &mut actual);
    actual
}

pub(super) fn collect_syntax_capture_effects_block(
    body: &SyntaxBlock,
    binding_names: &HashSet<String>,
    actual: &mut HashMap<String, DataEffect>,
) {
    for statement in &body.statements {
        match statement {
            SyntaxStmt::Assign(stmt) => {
                if let Some(name) = syntax_place_base(&stmt.target)
                    && binding_names.contains(&name)
                {
                    merge_syntax_capture_effect(actual, &name, DataEffect::Mut);
                }
                collect_syntax_capture_effects_expr(&stmt.target, binding_names, actual);
                collect_syntax_capture_effects_expr(&stmt.value, binding_names, actual);
            }
            SyntaxStmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_syntax_capture_effects_expr(value, binding_names, actual);
                }
            }
            SyntaxStmt::LetElse(stmt) => {
                collect_syntax_capture_effects_expr(&stmt.value, binding_names, actual);
                collect_syntax_capture_effects_block(&stmt.else_body, binding_names, actual);
            }
            SyntaxStmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_syntax_capture_effects_expr(value, binding_names, actual);
                }
            }
            SyntaxStmt::With(stmt) => {
                collect_syntax_capture_effects_expr(&stmt.resource, binding_names, actual);
                collect_syntax_capture_effects_block(&stmt.body, binding_names, actual);
            }
            SyntaxStmt::If(stmt) => {
                collect_syntax_capture_effects_expr(&stmt.condition, binding_names, actual);
                collect_syntax_capture_effects_block(&stmt.then_body, binding_names, actual);
                if let Some(else_body) = &stmt.else_body {
                    collect_syntax_capture_effects_block(else_body, binding_names, actual);
                }
            }
            SyntaxStmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_syntax_capture_effects_expr(condition, binding_names, actual);
                }
                collect_syntax_capture_effects_block(&stmt.body, binding_names, actual);
            }
            SyntaxStmt::For(stmt) => {
                collect_syntax_capture_effects_expr(&stmt.iterable, binding_names, actual);
                collect_syntax_capture_effects_block(&stmt.body, binding_names, actual);
            }
            SyntaxStmt::Match(stmt) => {
                collect_syntax_capture_effects_expr(&stmt.value, binding_names, actual);
                for arm in &stmt.arms {
                    collect_syntax_capture_effects_block(&arm.body, binding_names, actual);
                }
            }
            SyntaxStmt::TaskGroup(stmt) => {
                collect_syntax_capture_effects_block(&stmt.body, binding_names, actual);
            }
            SyntaxStmt::Select(stmt) => {
                for arm in &stmt.arms {
                    collect_syntax_capture_effects_expr(&arm.operation, binding_names, actual);
                    collect_syntax_capture_effects_block(&arm.body, binding_names, actual);
                }
            }
            SyntaxStmt::Expr(expr) => {
                collect_syntax_capture_effects_expr(expr, binding_names, actual);
            }
            SyntaxStmt::MalformedWith(_)
            | SyntaxStmt::MalformedIf(_)
            | SyntaxStmt::MalformedLoop(_)
            | SyntaxStmt::MalformedFor(_)
            | SyntaxStmt::MalformedMatch(_)
            | SyntaxStmt::Break(_)
            | SyntaxStmt::Continue(_)
            | SyntaxStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn collect_syntax_capture_effects_expr(
    expr: &Expr,
    binding_names: &HashSet<String>,
    actual: &mut HashMap<String, DataEffect>,
) {
    match expr {
        Expr::Ident(name, _) => {
            if binding_names.contains(name) {
                merge_syntax_capture_effect(actual, name, DataEffect::Read);
            }
        }
        Expr::Effect { effect, value, .. } => {
            if let Some(name) = syntax_place_base(value)
                && binding_names.contains(&name)
            {
                merge_syntax_capture_effect(actual, &name, *effect);
            }
            collect_syntax_capture_effects_expr(value, binding_names, actual);
        }
        Expr::Manage { value, .. } => {
            if let Some(name) = syntax_place_base(value)
                && binding_names.contains(&name)
            {
                merge_syntax_capture_effect(actual, &name, DataEffect::Take);
            }
            collect_syntax_capture_effects_expr(value, binding_names, actual);
        }
        Expr::Closure { body, .. } => {
            collect_syntax_capture_effects_block(body, binding_names, actual);
        }
        Expr::Binary { left, right, .. } => {
            collect_syntax_capture_effects_expr(left, binding_names, actual);
            collect_syntax_capture_effects_expr(right, binding_names, actual);
        }
        Expr::Field { base, .. } => {
            collect_syntax_capture_effects_expr(base, binding_names, actual)
        }
        Expr::Index { base, index, .. } => {
            collect_syntax_capture_effects_expr(base, binding_names, actual);
            collect_syntax_capture_effects_expr(index, binding_names, actual);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_syntax_capture_effects_expr(&arg.value, binding_names, actual);
            }
        }
        Expr::Spawn { value, .. } | Expr::Await { value, .. } | Expr::Try { value, .. } => {
            collect_syntax_capture_effects_expr(value, binding_names, actual);
        }
        Expr::Match { value, arms, .. } => {
            collect_syntax_capture_effects_expr(value, binding_names, actual);
            for arm in arms {
                collect_syntax_capture_effects_block(&arm.body, binding_names, actual);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_syntax_capture_effects_expr(&entry.key, binding_names, actual);
                collect_syntax_capture_effects_expr(&entry.value, binding_names, actual);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_syntax_capture_effects_expr(&field.value, binding_names, actual);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_syntax_capture_effects_expr(item, binding_names, actual);
            }
        }
        Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn syntax_place_base(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => syntax_place_base(base),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => syntax_place_base(value),
        _ => None,
    }
}

pub(super) fn merge_syntax_capture_effect(
    actual: &mut HashMap<String, DataEffect>,
    name: &str,
    effect: DataEffect,
) {
    let current = actual.get(name).copied().unwrap_or(DataEffect::Read);
    let next = match (current, effect) {
        (DataEffect::Take, _) | (_, DataEffect::Take) => DataEffect::Take,
        (DataEffect::Mut, _) | (_, DataEffect::Mut) => DataEffect::Mut,
        _ => DataEffect::Read,
    };
    actual.insert(name.to_string(), next);
}

pub(super) fn param_effect_from_data_effect_syntax(effect: DataEffect) -> ParamEffect {
    match effect {
        DataEffect::Read => ParamEffect::Read,
        DataEffect::Mut => ParamEffect::Mut,
        DataEffect::Take => ParamEffect::Take,
    }
}
