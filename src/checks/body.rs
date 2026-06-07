use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{
    CallResolution, FieldInfo, HirBindingKind, HirBlock, HirCallArg, HirExpr, HirMatchArm, HirStmt,
    HirTypeKind, ParamEffect, ResolvedCalleeKind,
};
use crate::syntax::ast::{
    Block as SyntaxBlock, Callee, DataEffect, Expr, FileFeature, FunctionDecl, Item,
    MatchFieldPattern, MatchLiteral, MatchPattern, Stmt as SyntaxStmt, TypeRef,
};

use super::local::{
    BodyState, FreshReturnIssue, FreshReturnIssueKind, LocalAnalysis, ManagedToLocalUse, MovedUse,
    ResourceEscapeKind, RetainedClosureCapture, RetainedLocalUse, TakeHandleField,
    is_copy_type_name, merge_if_state, merge_loop_state,
};

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let functions: Vec<FunctionDecl> = analyzer
        .syntax_program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function.clone()),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect();

    for function in functions {
        check_explicit_closure_capture_effects_syntax(analyzer, &function);
        analyzer.async_let_names.clear();
        let hir_body = analyzer.hir.function_body(&function.name).cloned();
        let local_analysis = LocalAnalysis::new(hir_body.as_ref());
        check_managed_to_local_uses(analyzer, &local_analysis);
        check_moved_uses(analyzer, &local_analysis);
        check_retained_local_uses(analyzer, &local_analysis);
        check_retained_closure_captures(analyzer, &local_analysis);
        check_take_handle_fields(analyzer, &local_analysis);
        check_fresh_returns(analyzer, &local_analysis, &function);
        if let Some(body) = &hir_body {
            if let Some(block) = body.block.as_ref() {
                check_await_placement(analyzer, block, function.is_async);
            }
            check_resource_pool_bindings(analyzer, body);
            check_local_class_bindings(analyzer, body);
            if let Some(block) = body.block.as_ref() {
                check_resource_pool_discards(analyzer, block, &mut Vec::new());
                let bindings: std::collections::HashMap<String, HirBindingKind> = body
                    .bindings
                    .iter()
                    .map(|binding| (binding.name.clone(), binding.kind))
                    .collect();
                check_lazy_factory_captures_block(analyzer, block, &bindings);
                let binding_names = bindings.keys().cloned().collect::<HashSet<_>>();
                check_explicit_closure_captures_block(analyzer, block, &binding_names);
            }
        }
        let mut state = local_analysis.initial_state();
        if let Some(block) = hir_body.as_ref().and_then(|body| body.block.as_ref()) {
            let return_error_type = function
                .return_ty
                .as_ref()
                .and_then(result_error_type_ref_name);
            check_try_error_types(analyzer, block, return_error_type.as_deref());
            check_block(
                analyzer,
                &local_analysis,
                block,
                &mut state,
                true,
                &HashSet::new(),
            );
        }
    }
}

fn check_explicit_closure_captures_block(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    binding_names: &HashSet<String>,
) {
    for statement in &block.statements {
        check_explicit_closure_captures_stmt(analyzer, statement, binding_names);
    }
}

fn check_explicit_closure_captures_stmt(
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
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
            check_explicit_closure_captures_expr(analyzer, expr, binding_names)
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn check_explicit_closure_captures_expr(
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
        | HirExpr::Unknown(_) => {}
    }
}

fn check_one_explicit_closure_capture_contract(
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
            Some((declared_effect, span)) if declared_effect != effect => {
                explicit_closure_capture_contract_diagnostic(
                    analyzer,
                    name,
                    *declared_effect,
                    *effect,
                    span.clone(),
                );
            }
            Some(_) => {}
            None => {
                explicit_closure_missing_capture_diagnostic(
                    analyzer,
                    name,
                    *effect,
                    body.span.clone(),
                );
            }
        }
    }
    for (name, (_, span)) in declared {
        if !actual.contains_key(&name) {
            explicit_closure_unused_capture_diagnostic(analyzer, &name, span);
        }
    }
}

fn explicit_closure_actual_captures(
    body: &HirBlock,
    params: &HashSet<String>,
    binding_names: &HashSet<String>,
) -> HashMap<String, ParamEffect> {
    let mut uses = HashSet::new();
    collect_block_uses(body, &mut uses);
    for param in params {
        uses.remove(param);
    }
    let mut actual = uses
        .into_iter()
        .filter(|name| binding_names.contains(name))
        .map(|name| (name, ParamEffect::Read))
        .collect::<HashMap<_, _>>();

    let bound = params.clone();
    let mut accesses = Vec::new();
    collect_closure_effect_accesses_block(body, &bound, &mut accesses);
    for access in accesses {
        if binding_names.contains(&access.path.base) {
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

fn strongest_capture_effect(left: ParamEffect, right: ParamEffect) -> ParamEffect {
    match (left, right) {
        (ParamEffect::Take, _) | (_, ParamEffect::Take) => ParamEffect::Take,
        (ParamEffect::Mut, _) | (_, ParamEffect::Mut) => ParamEffect::Mut,
        _ => ParamEffect::Read,
    }
}

fn check_local_class_bindings(analyzer: &mut Analyzer<'_>, body: &crate::hir::HirFunctionBody) {
    for binding in &body.bindings {
        if binding.kind == HirBindingKind::LocalLet
            && binding.type_name.as_deref().is_some_and(|type_name| {
                analyzer.hir.type_kind(type_name) == Some(HirTypeKind::Class)
            })
        {
            local_class_binding_diagnostic(analyzer, &binding.name, binding.span.clone());
        }
    }
}

fn check_explicit_closure_capture_effects_syntax(
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

fn collect_syntax_block_bindings(block: &SyntaxBlock, names: &mut HashSet<String>) {
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

fn collect_syntax_expr_bindings(expr: &Expr, names: &mut HashSet<String>) {
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
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn check_explicit_closure_capture_effects_syntax_block(
    analyzer: &mut Analyzer<'_>,
    block: &SyntaxBlock,
    binding_names: &HashSet<String>,
) {
    for statement in &block.statements {
        check_explicit_closure_capture_effects_syntax_stmt(analyzer, statement, binding_names);
    }
}

fn check_explicit_closure_capture_effects_syntax_stmt(
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

fn check_explicit_closure_capture_effects_syntax_expr(
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
                    if let Some(actual_effect) = actual.get(&capture.name)
                        && capture.effect != *actual_effect
                    {
                        explicit_closure_capture_contract_diagnostic(
                            analyzer,
                            &capture.name,
                            param_effect_from_data_effect_syntax(capture.effect),
                            param_effect_from_data_effect_syntax(*actual_effect),
                            capture.span.clone(),
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
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn syntax_closure_actual_capture_effects(
    body: &SyntaxBlock,
    binding_names: &HashSet<String>,
) -> HashMap<String, DataEffect> {
    let mut actual = HashMap::new();
    collect_syntax_capture_effects_block(body, binding_names, &mut actual);
    actual
}

fn collect_syntax_capture_effects_block(
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

fn collect_syntax_capture_effects_expr(
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
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn syntax_place_base(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => syntax_place_base(base),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => syntax_place_base(value),
        _ => None,
    }
}

fn merge_syntax_capture_effect(
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

fn param_effect_from_data_effect_syntax(effect: DataEffect) -> ParamEffect {
    match effect {
        DataEffect::Read => ParamEffect::Read,
        DataEffect::Mut => ParamEffect::Mut,
        DataEffect::Take => ParamEffect::Take,
    }
}

fn check_resource_pool_bindings(analyzer: &mut Analyzer<'_>, body: &crate::hir::HirFunctionBody) {
    for binding in &body.bindings {
        if !binding
            .type_name
            .as_deref()
            .is_some_and(is_resource_pool_type)
        {
            continue;
        }
        let binding_is_local = binding.kind == HirBindingKind::LocalLet
            || (binding.kind == HirBindingKind::Param
                && matches!(binding.effect, Some(ParamEffect::Mut | ParamEffect::Take)));
        if !binding_is_local {
            resource_pool_not_local_diagnostic(analyzer, &binding.name, binding.span.clone());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Fallthrough,
    Return,
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallPlaceAccess {
    effect: ParamEffect,
    path: PlacePath,
    moves_path: bool,
    base_is_local: bool,
    span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlacePath {
    base: String,
    components: Vec<String>,
    has_index: bool,
    crosses_handle: bool,
}

fn check_block(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    block: &HirBlock,
    state: &mut BodyState,
    check_resource_contexts: bool,
    continuation_uses: &HashSet<String>,
) -> Flow {
    let live_after_statements = block_live_after_statements(block, continuation_uses);
    for (index, statement) in block.statements.iter().enumerate() {
        // Track async let names so await checks can recognize pending bindings
        if let HirStmt::Let {
            is_async: true,
            name,
            ..
        } = statement
        {
            analyzer.async_let_names.push(name.clone());
        }
        let live_after = live_after_statements
            .get(index)
            .unwrap_or(continuation_uses);
        let flow = check_stmt_semantics(
            analyzer,
            local_analysis,
            statement,
            state,
            check_resource_contexts,
            live_after,
        );
        apply_stmt_effects(statement, state);
        if flow != Flow::Fallthrough {
            return flow;
        }
    }
    Flow::Fallthrough
}

fn block_live_after_statements(
    block: &HirBlock,
    continuation_uses: &HashSet<String>,
) -> Vec<HashSet<String>> {
    let mut live_after = vec![HashSet::new(); block.statements.len()];
    let mut used = continuation_uses.clone();
    for (index, statement) in block.statements.iter().enumerate().rev() {
        live_after[index] = used.clone();
        collect_stmt_uses(statement, &mut used);
        remove_stmt_bindings(statement, &mut used);
    }
    live_after
}

fn collect_stmt_uses(statement: &HirStmt, uses: &mut HashSet<String>) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_expr_uses(value, uses);
            }
        }
        HirStmt::With { resource, body, .. } => {
            collect_expr_uses(resource, uses);
            collect_block_uses(body, uses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_uses(condition, uses);
            collect_block_uses(then_body, uses);
            if let Some(else_body) = else_body {
                collect_block_uses(else_body, uses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_uses(condition, uses);
            }
            collect_block_uses(body, uses);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_uses(iterable, uses);
            collect_block_uses(body, uses);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_uses(value, uses);
            for arm in arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_uses(&arm.operation, uses);
                collect_block_uses(&arm.body, uses);
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => collect_expr_uses(expr, uses),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_block_uses(block: &HirBlock, uses: &mut HashSet<String>) {
    let mut block_uses = HashSet::new();
    for statement in block.statements.iter().rev() {
        collect_stmt_uses(statement, &mut block_uses);
        remove_stmt_bindings(statement, &mut block_uses);
    }
    for name in block_uses {
        uses.insert(name);
    }
}

fn collect_expr_uses(expr: &HirExpr, uses: &mut HashSet<String>) {
    match expr {
        HirExpr::Ident { name, .. } => {
            if !is_builtin_value_ident(name) {
                uses.insert(name.clone());
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_uses(left, uses);
            collect_expr_uses(right, uses);
        }
        HirExpr::Field { base, .. } => collect_expr_uses(base, uses),
        HirExpr::Index { base, index, .. } => {
            collect_expr_uses(base, uses);
            collect_expr_uses(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_uses(&arg.value, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_uses(value, uses),
        HirExpr::Closure { body, .. } => collect_block_uses(body, uses),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_uses(value, uses);
            for arm in arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_uses(&entry.key, uses);
                collect_expr_uses(&entry.value, uses);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_await_operand_live_uses(expr: &HirExpr, uses: &mut HashSet<String>) {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Take,
            ..
        }
        | HirExpr::Manage { .. } => {}
        HirExpr::Ident { name, .. } => {
            if !is_builtin_value_ident(name) {
                uses.insert(name.clone());
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_await_operand_live_uses(left, uses);
            collect_await_operand_live_uses(right, uses);
        }
        HirExpr::Field { base, .. } => collect_await_operand_live_uses(base, uses),
        HirExpr::Index { base, index, .. } => {
            collect_await_operand_live_uses(base, uses);
            collect_await_operand_live_uses(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_await_operand_live_uses(&arg.value, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_await_operand_live_uses(value, uses),
        HirExpr::Closure { body, .. } => collect_block_uses(body, uses),
        HirExpr::Match { value, arms, .. } => {
            collect_await_operand_live_uses(value, uses);
            for arm in arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_await_operand_live_uses(&entry.key, uses);
                collect_await_operand_live_uses(&entry.value, uses);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn is_builtin_value_ident(name: &str) -> bool {
    matches!(name, "Unit" | "true" | "false" | "null")
}

fn remove_stmt_bindings(statement: &HirStmt, uses: &mut HashSet<String>) {
    match statement {
        HirStmt::Let { name, .. } | HirStmt::For { binding: name, .. } => {
            uses.remove(name);
        }
        HirStmt::With { binding, .. } => {
            uses.remove(binding);
        }
        HirStmt::Match { arms, .. } => {
            for arm in arms {
                for binding in arm.pattern.binding_names() {
                    uses.remove(binding);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                if arm.binding != "_" {
                    uses.remove(&arm.binding);
                }
            }
        }
        HirStmt::Return { .. }
        | HirStmt::If { .. }
        | HirStmt::Loop { .. }
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn check_stmt_semantics(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    statement: &HirStmt,
    state: &mut BodyState,
    check_resource_contexts: bool,
    live_after: &HashSet<String>,
) -> Flow {
    match statement {
        HirStmt::Let {
            kind,
            value,
            is_async,
            span,
            ..
        } => {
            let mut merged_entry_state;
            let stmt_state = if let Some(entry_state) = local_analysis.flow_entry_state(span) {
                merged_entry_state = entry_state.clone();
                merged_entry_state
                    .read_views
                    .extend(state.read_views.iter().cloned());
                &merged_entry_state
            } else {
                state
            };
            if *kind == HirBindingKind::ManagedLet {
                check_managed_closure_captures(analyzer, local_analysis, span, stmt_state);
            }
            if let Some(value) = value {
                if *is_async {
                    // async let consumes the async call (produces a pending)
                    check_expr_semantics_with_context(
                        analyzer,
                        local_analysis,
                        value,
                        stmt_state,
                        false,
                        true,
                        live_after,
                    );
                } else {
                    check_expr_semantics(analyzer, local_analysis, value, stmt_state, live_after);
                }
                if check_resource_contexts {
                    check_resource_pool_lease_expr(analyzer, value, false);
                    check_resource_producer_expr(analyzer, value, false);
                }
            }

            Flow::Fallthrough
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_expr_semantics(analyzer, local_analysis, value, state, live_after);
                if check_resource_contexts {
                    check_resource_pool_lease_expr(analyzer, value, false);
                    check_resource_producer_expr(analyzer, value, false);
                }
            }
            Flow::Return
        }
        HirStmt::With {
            resource,
            body,
            span,
            binding,
            ..
        } => {
            check_expr_semantics(analyzer, local_analysis, resource, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, resource, true);
                check_result_resource_with_has_try(analyzer, resource);
                check_resource_producer_expr(analyzer, resource, true);
                check_resource_escape(analyzer, local_analysis, span);
            }
            if check_resource_contexts
                && let Some(pool_path) = resource_pool_borrow_pool_path(resource)
            {
                check_resource_pool_active_lease_block(analyzer, &pool_path, body);
            }
            let mut scoped_state = state.clone();
            scoped_state.bind_resource(binding.clone());
            check_block(
                analyzer,
                local_analysis,
                body,
                &mut scoped_state,
                check_resource_contexts,
                live_after,
            )
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_bool_condition(analyzer, condition, "if");
            check_expr_semantics(analyzer, local_analysis, condition, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, condition, false);
                check_resource_producer_expr(analyzer, condition, false);
            }
            apply_expr_effects(condition, state);

            let base_state = state.clone();
            let mut then_state = base_state.clone();
            let then_flow = check_block(
                analyzer,
                local_analysis,
                then_body,
                &mut then_state,
                check_resource_contexts,
                live_after,
            );

            let else_branch = else_body.as_ref().map(|else_body| {
                let mut else_state = base_state.clone();
                let else_flow = check_block(
                    analyzer,
                    local_analysis,
                    else_body,
                    &mut else_state,
                    check_resource_contexts,
                    live_after,
                );
                (else_state, else_flow)
            });

            merge_if_state(state, &base_state, then_state, then_flow, else_branch)
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_bool_condition(analyzer, condition, "while");
                check_expr_semantics(analyzer, local_analysis, condition, state, live_after);
                if check_resource_contexts {
                    check_resource_pool_lease_expr(analyzer, condition, false);
                    check_resource_producer_expr(analyzer, condition, false);
                }
                apply_expr_effects(condition, state);
            }

            let base_state = state.clone();
            let mut body_state = base_state.clone();
            let body_flow = check_block(
                analyzer,
                local_analysis,
                body,
                &mut body_state,
                check_resource_contexts,
                live_after,
            );

            merge_loop_state(
                state,
                &base_state,
                body_state,
                body_flow,
                condition.is_some(),
            )
        }
        HirStmt::For {
            binding,
            iterable,
            iterable_type_name,
            item_type_name,
            is_async,
            body,
            ..
        } => {
            check_for_iterable_type(analyzer, iterable, iterable_type_name.as_deref(), *is_async);
            check_expr_semantics(analyzer, local_analysis, iterable, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, iterable, false);
                check_resource_producer_expr(analyzer, iterable, false);
            }
            apply_expr_effects(iterable, state);

            let base_state = state.clone();
            let mut body_state = base_state.clone();
            if let Some(item_type_name) = item_type_name {
                body_state.record_type(binding.clone(), item_type_name.clone());
                if analyzer.hir.type_kind(type_root_name(item_type_name))
                    == Some(HirTypeKind::Class)
                {
                    body_state.bind_managed(binding.clone());
                } else if !is_async && !is_copy_type_name(item_type_name) {
                    body_state.bind_read_view(binding.clone());
                }
            }
            let body_flow = check_block(
                analyzer,
                local_analysis,
                body,
                &mut body_state,
                check_resource_contexts,
                live_after,
            );
            merge_loop_state(state, &base_state, body_state, body_flow, true)
        }
        HirStmt::Match {
            value,
            scrutinee_effect,
            arms,
            ..
        } => {
            check_match_scrutinee_type(analyzer, value);
            check_match_patterns_match_scrutinee(analyzer, value, arms);
            check_match_pattern_effects(analyzer, value, *scrutinee_effect, arms);
            if *scrutinee_effect == Some(DataEffect::Take) {
                check_take_operand_is_local(analyzer, value, &arm_span(arms), state);
            }
            check_expr_semantics(analyzer, local_analysis, value, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, value, false);
                check_resource_producer_expr(analyzer, value, false);
            }
            apply_expr_effects(value, state);
            apply_match_scrutinee_effect(*scrutinee_effect, value, &arm_span(arms), state);

            let base_state = state.clone();
            let mut all_return = !arms.is_empty();
            for arm in arms {
                let mut arm_state = base_state.clone();
                let flow = check_block(
                    analyzer,
                    local_analysis,
                    &arm.body,
                    &mut arm_state,
                    check_resource_contexts,
                    live_after,
                );
                all_return &= flow == Flow::Return;
            }
            if all_return {
                Flow::Return
            } else {
                Flow::Fallthrough
            }
        }
        HirStmt::Select { arms, .. } => {
            // Every arm operation is constructed and polled, so their effects
            // apply to the shared state first. Then exactly one body runs, so the
            // bodies are mutually-exclusive branches off that common base — like
            // `match` arms — which avoids false cross-arm ownership conflicts.
            for arm in arms {
                check_expr_semantics(analyzer, local_analysis, &arm.operation, state, live_after);
                if check_resource_contexts {
                    check_resource_pool_lease_expr(analyzer, &arm.operation, false);
                    check_resource_producer_expr(analyzer, &arm.operation, false);
                }
                apply_expr_effects(&arm.operation, state);
            }
            let base_state = state.clone();
            let mut all_return = !arms.is_empty();
            for arm in arms {
                let mut arm_state = base_state.clone();
                if arm.binding != "_" {
                    arm_state.bind_local(arm.binding.clone());
                }
                let flow = check_block(
                    analyzer,
                    local_analysis,
                    &arm.body,
                    &mut arm_state,
                    check_resource_contexts,
                    live_after,
                );
                all_return &= flow == Flow::Return;
            }
            if all_return {
                Flow::Return
            } else {
                Flow::Fallthrough
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
            check_expr_semantics(analyzer, local_analysis, expr, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, expr, false);
                check_resource_producer_expr(analyzer, expr, false);
            }
            Flow::Fallthrough
        }
        HirStmt::Break(_) => Flow::Break,
        HirStmt::Continue(_) => Flow::Continue,
        HirStmt::Unknown(_) => Flow::Fallthrough,
    }
}

fn apply_stmt_effects(statement: &HirStmt, state: &mut BodyState) {
    match statement {
        HirStmt::Let {
            kind,
            name,
            value,
            type_name,
            ..
        } => {
            match kind {
                HirBindingKind::ManagedLet => state.bind_managed(name.clone()),
                HirBindingKind::LocalLet => state.bind_local(name.clone()),
                HirBindingKind::Param => {}
            }
            if let Some(type_name) = type_name {
                state.record_type(name.clone(), type_name.clone());
            }
            if let Some(value) = value {
                apply_expr_effects(value, state);
            }
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                apply_expr_effects(value, state);
            }
        }
        HirStmt::With { resource, .. } => {
            apply_expr_effects(resource, state);
        }
        HirStmt::If { .. } => {}
        HirStmt::Loop { .. } => {}
        HirStmt::For { iterable, .. } => apply_expr_effects(iterable, state),
        HirStmt::Match { value, .. } => apply_expr_effects(value, state),
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                apply_expr_effects(&arm.operation, state);
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
            apply_expr_effects(expr, state)
        }
        HirStmt::Break(_) | HirStmt::Continue(_) => {}
        HirStmt::Unknown(_) => {}
    }
}

fn check_expr_semantics(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    expr: &HirExpr,
    state: &BodyState,
    live_after: &HashSet<String>,
) {
    check_expr_semantics_with_context(
        analyzer,
        local_analysis,
        expr,
        state,
        false,
        false,
        live_after,
    );
}

fn check_bool_condition(analyzer: &mut Analyzer<'_>, expr: &HirExpr, construct: &str) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    if type_name == "Bool" {
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("{construct} condition has type `{type_name}`, expected `Bool`."),
            hir_expr_span(expr).clone(),
            "control-flow type mismatch",
        )
        .with_cause("RSScript control-flow conditions are explicit `Bool` values; non-empty strings, numbers, and managed handles do not coerce to truthy or falsey values.")
        .with_fix(
            "use_bool_condition",
            "Compare explicitly or call a function that returns `Bool`.",
            "manual",
        ),
    );
}

fn check_for_iterable_type(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    type_name: Option<&str>,
    is_async: bool,
) {
    let Some(type_name) = type_name else {
        return;
    };
    let bare_type_name = strip_fresh_type(type_name);
    if (!is_async && list_element_type(bare_type_name).is_some())
        || (is_async && stream_item_type(bare_type_name).is_some())
    {
        return;
    }
    let expected = if is_async { "Stream<T>" } else { "List<T>" };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("for iterable has type `{type_name}`, expected `{expected}`."),
            hir_expr_span(expr).clone(),
            "control-flow type mismatch",
        )
        .with_cause(if is_async {
            "RSScript `await for` iterates `Stream<T>` values by repeatedly awaiting `Stream.next`."
        } else {
            "RSScript v0.6 `for` iteration is limited to `List<T>` so loop ownership and review metadata stay explicit."
        })
        .with_fix(
            if is_async { "iterate_stream" } else { "iterate_list" },
            if is_async {
                "Iterate a `Stream<T>` value or convert the input to a Stream before the loop."
            } else {
                "Iterate a `List<T>` value or convert the input to a List before the loop."
            },
            "manual",
        ),
    );
}

fn check_match_scrutinee_type(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    if type_root_name(type_name) == "Option" || type_root_name(type_name) == "Result" {
        return;
    }
    if matches!(type_name, "Int" | "String" | "Bool") {
        return;
    }
    // Allow matching on user-defined sum/struct/class types.
    if matches!(
        analyzer.hir.type_kind(type_name),
        Some(HirTypeKind::Sum | HirTypeKind::Struct | HirTypeKind::Class)
    ) {
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("match scrutinee has type `{type_name}`, expected `Option<T>`, `Result<T, E>`, a declared sum/struct/class type, or an `Int`/`String`/`Bool` literal match."),
            hir_expr_span(expr).clone(),
            "control-flow type mismatch",
        )
        .with_cause("RSScript v0.6 `match` is limited to review-visible `Option`, `Result`, declared sum/struct/class patterns, and simple scalar literal dispatch.")
        .with_fix(
            "match_option_or_result",
            "Match an `Option<T>`, `Result<T, E>`, declared sum value, or scalar literal value; otherwise rewrite this branch as `if`.",
            "manual",
        ),
    );
}

fn check_match_patterns_match_scrutinee(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    arms: &[HirMatchArm],
) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    for arm in arms {
        check_match_pattern_matches_type(analyzer, &arm.pattern, type_name);
    }
}

fn check_match_pattern_matches_type(
    analyzer: &mut Analyzer<'_>,
    pattern: &MatchPattern,
    type_name: &str,
) {
    let root = type_root_name(type_name);
    match pattern {
        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => {}
        MatchPattern::Literal { value, span } => {
            let literal_type = match value {
                MatchLiteral::Int(_) => "Int",
                MatchLiteral::String(_) => "String",
                MatchLiteral::Bool(_) => "Bool",
            };
            if literal_type != type_name {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::CONTROL_FLOW_TYPE_MISMATCH,
                        format!("literal match pattern cannot match scrutinee type `{type_name}`."),
                        span.clone(),
                        "match literal type mismatch",
                    )
                    .with_cause(
                        "Literal patterns are only allowed when matching `Int`, `String`, or `Bool` values.",
                    ),
                );
            }
        }
        MatchPattern::Variant {
            name,
            binding,
            span,
        } if root == "Option" => {
            if !matches!(name.as_str(), "Some" | "None") {
                push_match_variant_type_mismatch(
                    analyzer,
                    name,
                    type_name,
                    &["Some".to_string(), "None".to_string()],
                    span,
                );
                return;
            }
            if name == "Some"
                && let Some(binding) = binding
                && let Some(inner) =
                    type_arg_names(type_name).and_then(|args| args.first().copied())
            {
                check_match_pattern_matches_type(analyzer, binding, inner);
            }
        }
        MatchPattern::Variant {
            name,
            binding,
            span,
        } if root == "Result" => {
            if !matches!(name.as_str(), "Ok" | "Err") {
                push_match_variant_type_mismatch(
                    analyzer,
                    name,
                    type_name,
                    &["Ok".to_string(), "Err".to_string()],
                    span,
                );
                return;
            }
            if let Some(binding) = binding
                && let Some(args) = type_arg_names(type_name)
            {
                let payload_type = if name == "Ok" {
                    args.first()
                } else {
                    args.get(1)
                };
                if let Some(payload_type) = payload_type {
                    check_match_pattern_matches_type(analyzer, binding, payload_type);
                }
            }
        }
        MatchPattern::Variant {
            name,
            binding,
            span,
        } => {
            let Some((_, fields)) = pattern_sum_variant_fields(analyzer, root, name) else {
                let allowed = allowed_sum_variant_names(analyzer, root);
                if allowed.is_empty() {
                    push_variant_or_struct_cannot_match(analyzer, name, type_name, span);
                } else {
                    push_match_variant_type_mismatch(analyzer, name, type_name, &allowed, span);
                }
                return;
            };
            if let Some(binding) = binding
                && let Some(field) = fields.first()
            {
                check_match_pattern_matches_type(analyzer, binding, &field.type_name);
            }
        }
        MatchPattern::Struct {
            name, fields, span, ..
        } => {
            let declared = if name == root
                && matches!(
                    analyzer.hir.type_kind(root),
                    Some(HirTypeKind::Struct | HirTypeKind::Class)
                ) {
                analyzer
                    .hir
                    .type_info(root)
                    .map(|info| info.fields.values().cloned().collect::<Vec<FieldInfo>>())
            } else {
                pattern_sum_variant_fields(analyzer, root, name).map(|(_, fields)| fields)
            };
            let Some(declared) = declared else {
                push_variant_or_struct_cannot_match(analyzer, name, type_name, span);
                return;
            };
            for field in fields {
                if let Some(pattern) = &field.pattern
                    && let Some(field_info) = declared
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                {
                    check_match_pattern_matches_type(analyzer, pattern, &field_info.type_name);
                }
            }
        }
    }
}

fn push_variant_or_struct_cannot_match(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    type_name: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("match pattern `{name}` cannot match scrutinee type `{type_name}`."),
            span.clone(),
            "match pattern type mismatch",
        )
        .with_cause("RSScript match patterns must belong to the scrutinee's type."),
    );
}

fn push_match_variant_type_mismatch(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    type_name: &str,
    allowed: &[String],
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("match pattern `{name}` cannot match scrutinee type `{type_name}`."),
            span.clone(),
            "match variant type mismatch",
        )
        .with_cause("RSScript match patterns must belong to the scrutinee's type.")
        .with_fix(
            "match_matching_variant_family",
            format!(
                "Use variants of `{}`: {}.",
                type_root_name(type_name),
                allowed.join(", ")
            ),
            "manual",
        ),
    );
}

fn allowed_sum_variant_names(analyzer: &Analyzer<'_>, root: &str) -> Vec<String> {
    match root {
        "Option" => return vec!["Some".to_string(), "None".to_string()],
        "Result" => return vec!["Ok".to_string(), "Err".to_string()],
        _ => {}
    }
    analyzer
        .syntax_program
        .items
        .iter()
        .find_map(|item| match item {
            Item::SumType(sum) if sum.name == root => Some(
                sum.variants
                    .iter()
                    .map(|variant| variant.name.clone())
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn pattern_sum_variant_fields(
    analyzer: &Analyzer<'_>,
    root: &str,
    variant_name: &str,
) -> Option<(String, Vec<FieldInfo>)> {
    analyzer
        .hir
        .sum_type_for_variant(variant_name)
        .filter(|sum| *sum == root)?;
    analyzer
        .hir
        .sum_variant_fields(variant_name)
        .map(<[FieldInfo]>::to_vec)
        .map(|fields| (root.to_string(), fields))
}

fn check_match_pattern_effects(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    scrutinee_effect: Option<DataEffect>,
    arms: &[HirMatchArm],
) {
    let scrutinee_type = hir_expr_type_name(value).map(str::to_string);
    let managed_class_scrutinee = hir_expr_type_name(value)
        .map(type_root_name)
        .is_some_and(|root| analyzer.hir.type_kind(root) == Some(HirTypeKind::Class));
    if scrutinee_effect == Some(DataEffect::Take)
        && !analyzer.syntax_program.has_feature(FileFeature::Local)
        && let Some(first_arm) = arms.first()
    {
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::FEATURE_VIOLATION,
                "`match take` requires `features: local`.",
                first_arm.span.clone(),
                "missing local feature",
            )
            .with_cause("Taking fields out of a pattern is a local ownership capability.")
            .with_fix(
                "add_local_feature",
                "Add `features: local` or match the value with `read`.",
                "manual",
            ),
        );
    }
    for arm in arms {
        if matches!(arm.pattern, MatchPattern::Struct { .. }) && scrutinee_effect.is_none() {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::MISSING_DATA_EFFECT,
                    "structured match patterns require an explicit scrutinee effect.",
                    arm.span.clone(),
                    "missing match scrutinee effect",
                )
                .with_cause("A structured pattern projects fields from the scrutinee, so the source must spell `match read`, `match mut`, or `match take`.")
                .with_fix(
                    "spell_match_effect",
                    "Add `read`, `mut`, or `take` after `match`.",
                    "manual",
                ),
            );
        }
        check_pattern_field_effects(
            analyzer,
            scrutinee_type.as_deref(),
            scrutinee_effect,
            managed_class_scrutinee,
            &arm.pattern,
        );
        if let Some(guard) = &arm.guard
            && let Some((effect, span)) = first_mutating_effect_expr(guard)
        {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::READ_VIEW_MUTATION,
                    format!("match guard cannot use `{}`.", effect.as_str()),
                    span.clone(),
                    "guard mutation is not allowed",
                )
                .with_cause("A guard runs before the arm is selected and may only read pattern bindings.")
                .with_fix(
                    "make_guard_read_only",
                    "Move mutation into the selected arm body or rewrite the guard as a read-only predicate.",
                    "manual",
                ),
            );
        }
    }
}

fn check_pattern_field_effects(
    analyzer: &mut Analyzer<'_>,
    scrutinee_type: Option<&str>,
    scrutinee_effect: Option<DataEffect>,
    managed_class_scrutinee: bool,
    pattern: &MatchPattern,
) {
    let MatchPattern::Struct {
        name,
        fields,
        has_rest,
        span,
        ..
    } = pattern
    else {
        return;
    };
    check_struct_pattern_fields(analyzer, scrutinee_type, name, fields, *has_rest, span);
    let mut seen_fields: HashMap<&str, (DataEffect, Span)> = HashMap::new();
    for field in fields {
        let effective_effect = field.effect.unwrap_or(match scrutinee_effect {
            Some(DataEffect::Take) => DataEffect::Take,
            _ => DataEffect::Read,
        });
        if let Some((previous_effect, previous_span)) =
            seen_fields.insert(field.name.as_str(), (effective_effect, field.span.clone()))
        {
            let conflicts = matches!(previous_effect, DataEffect::Mut | DataEffect::Take)
                || matches!(effective_effect, DataEffect::Mut | DataEffect::Take);
            if conflicts {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::FIELD_PARTIAL_ACCESS_CONFLICT,
                        format!(
                            "pattern field `{}` is bound more than once with mutable or taking access.",
                            field.name
                        ),
                        field.span.clone(),
                        "pattern field conflict",
                    )
                    .with_cause(format!(
                        "The previous binding for `{}` was at {}:{}.",
                        field.name, previous_span.line, previous_span.column
                    ))
                    .with_fix(
                        "remove_overlapping_pattern_binding",
                        "Bind each mutable or taking field place at most once in a pattern.",
                        "manual",
                    ),
                );
            }
        }
        if field.ignored {
            continue;
        };
        let effect = effective_effect;
        if managed_class_scrutinee && matches!(effect, DataEffect::Mut | DataEffect::Take) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::READ_VIEW_MUTATION,
                    format!(
                        "managed pattern field `{}` cannot request `{}`.",
                        field.name,
                        effect.as_str()
                    ),
                    field.span.clone(),
                    "managed pattern field is read-only",
                )
                .with_cause("Managed class values are shared runtime objects; structured patterns expose only read views of their fields.")
                .with_fix(
                    "use_read_pattern",
                    "Use a read field binding and perform managed mutation through an explicit method.",
                    "manual",
                ),
            );
            continue;
        }
        let allowed = match scrutinee_effect {
            Some(DataEffect::Read) => effect == DataEffect::Read,
            Some(DataEffect::Mut) => matches!(effect, DataEffect::Read | DataEffect::Mut),
            Some(DataEffect::Take) => true,
            None => effect == DataEffect::Read,
        };
        if !allowed {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::READ_VIEW_MUTATION,
                    format!(
                        "field pattern `{}` requests `{}` from a weaker match scrutinee.",
                        field.name,
                        effect.as_str()
                    ),
                    field.span.clone(),
                    "pattern effect is not allowed",
                )
                .with_cause("Pattern binding effects are monotonic: a child field cannot request more authority than the scrutinee effect provides.")
                .with_fix(
                    "weaken_pattern_effect",
                    "Use `read` for this field or strengthen the match scrutinee effect when the value is local and mutable.",
                    "manual",
                ),
            );
        }
    }
}

fn check_struct_pattern_fields(
    analyzer: &mut Analyzer<'_>,
    scrutinee_type: Option<&str>,
    pattern_name: &str,
    fields: &[MatchFieldPattern],
    has_rest: bool,
    pattern_span: &Span,
) {
    let Some(declared_fields) = declared_pattern_fields(analyzer, scrutinee_type, pattern_name)
    else {
        return;
    };
    let declared_names: HashSet<&str> = declared_fields.iter().map(String::as_str).collect();
    let mut seen_fields: HashMap<&str, Span> = HashMap::new();
    for field in fields {
        if let Some(previous_span) = seen_fields.insert(field.name.as_str(), field.span.clone()) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::FIELD_PARTIAL_ACCESS_CONFLICT,
                    format!("pattern field `{}` is listed more than once.", field.name),
                    field.span.clone(),
                    "duplicate pattern field",
                )
                .with_cause(format!(
                    "The previous projection of `{}` was at {}:{}.",
                    field.name, previous_span.line, previous_span.column
                ))
                .with_fix(
                    "remove_duplicate_pattern_field",
                    "List each field at most once in a structured pattern.",
                    "manual",
                ),
            );
        }
        if !declared_names.contains(field.name.as_str()) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_FIELD,
                    format!("unknown field `{}` on type `{pattern_name}`.", field.name),
                    field.span.clone(),
                    "unknown field",
                )
                .with_cause("Structured match patterns may only project declared fields.")
                .with_fix(
                    "use_declared_pattern_field",
                    format!("Use a field declared on `{pattern_name}` or update the pattern."),
                    "manual",
                ),
            );
        }
    }
    if !has_rest && fields.len() < declared_fields.len() {
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::CONTROL_FLOW_TYPE_MISMATCH,
                format!("pattern `{pattern_name} {{ ... }}` omits fields without `..`."),
                fields
                    .last()
                    .map(|field| field.span.clone())
                    .unwrap_or_else(|| pattern_span.clone()),
                "pattern omits fields",
            )
            .with_cause("Omitted fields must be visible in review; write `..` when intentionally ignoring the rest.")
            .with_fix(
                "add_pattern_rest",
                format!("Write `{pattern_name} {{ ..., .. }}` when omitting fields."),
                "manual",
            ),
        );
    }
}

fn declared_pattern_fields(
    analyzer: &Analyzer<'_>,
    scrutinee_type: Option<&str>,
    pattern_name: &str,
) -> Option<Vec<String>> {
    let scrutinee_root = scrutinee_type.map(type_root_name)?;
    if analyzer
        .hir
        .sum_type_for_variant(pattern_name)
        .is_some_and(|sum| sum == scrutinee_root)
    {
        return analyzer
            .syntax_program
            .items
            .iter()
            .find_map(|item| match item {
                Item::SumType(sum) if sum.name == scrutinee_root => sum
                    .variants
                    .iter()
                    .find(|variant| variant.name == pattern_name)
                    .map(|variant| {
                        variant
                            .fields
                            .iter()
                            .map(|field| field.name.clone())
                            .collect()
                    }),
                _ => None,
            });
    }
    if pattern_name == scrutinee_root {
        return analyzer.hir.type_info(scrutinee_root).map(|type_info| {
            type_info
                .fields
                .values()
                .map(|field| field.name.clone())
                .collect()
        });
    }
    None
}

fn first_mutating_effect_expr(expr: &HirExpr) -> Option<(DataEffect, &crate::diagnostic::Span)> {
    match expr {
        HirExpr::Effect { effect, span, .. } if matches!(effect, ParamEffect::Mut) => {
            Some((DataEffect::Mut, span))
        }
        HirExpr::Effect { effect, span, .. } if matches!(effect, ParamEffect::Take) => {
            Some((DataEffect::Take, span))
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => first_mutating_effect_expr(value),
        HirExpr::Binary { left, right, .. } => {
            first_mutating_effect_expr(left).or_else(|| first_mutating_effect_expr(right))
        }
        HirExpr::Field { base, .. } => first_mutating_effect_expr(base),
        HirExpr::Index { base, index, .. } => {
            first_mutating_effect_expr(base).or_else(|| first_mutating_effect_expr(index))
        }
        HirExpr::Call { args, .. } => args
            .iter()
            .find_map(|arg| first_mutating_effect_expr(&arg.value)),
        HirExpr::Closure { body, .. } => {
            body.statements.iter().find_map(first_mutating_effect_stmt)
        }
        HirExpr::Match { value, arms, .. } => first_mutating_effect_expr(value).or_else(|| {
            arms.iter().find_map(|arm| {
                arm.guard
                    .as_ref()
                    .and_then(first_mutating_effect_expr)
                    .or_else(|| first_mutating_effect_block(&arm.body))
            })
        }),
        HirExpr::MapLiteral { entries, .. } => entries.iter().find_map(|entry| {
            first_mutating_effect_expr(&entry.key)
                .or_else(|| first_mutating_effect_expr(&entry.value))
        }),
        HirExpr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| first_mutating_effect_expr(&field.value)),
        HirExpr::ArrayLiteral { items, .. } => items.iter().find_map(first_mutating_effect_expr),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn first_mutating_effect_block(block: &HirBlock) -> Option<(DataEffect, &crate::diagnostic::Span)> {
    block.statements.iter().find_map(first_mutating_effect_stmt)
}

fn first_mutating_effect_stmt(stmt: &HirStmt) -> Option<(DataEffect, &crate::diagnostic::Span)> {
    match stmt {
        HirStmt::Let { value, .. } => value.as_ref().and_then(first_mutating_effect_expr),
        HirStmt::Return { value, .. } => value.as_ref().and_then(first_mutating_effect_expr),
        HirStmt::With { resource, body, .. } => {
            first_mutating_effect_expr(resource).or_else(|| first_mutating_effect_block(body))
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => first_mutating_effect_expr(condition)
            .or_else(|| first_mutating_effect_block(then_body))
            .or_else(|| else_body.as_ref().and_then(first_mutating_effect_block)),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(first_mutating_effect_expr)
            .or_else(|| first_mutating_effect_block(body)),
        HirStmt::For { iterable, body, .. } => {
            first_mutating_effect_expr(iterable).or_else(|| first_mutating_effect_block(body))
        }
        HirStmt::Match { value, arms, .. } => first_mutating_effect_expr(value).or_else(|| {
            arms.iter().find_map(|arm| {
                arm.guard
                    .as_ref()
                    .and_then(first_mutating_effect_expr)
                    .or_else(|| first_mutating_effect_block(&arm.body))
            })
        }),
        HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
            first_mutating_effect_expr(&arm.operation)
                .or_else(|| first_mutating_effect_block(&arm.body))
        }),
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => first_mutating_effect_expr(value),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => None,
    }
}

fn check_expr_semantics_with_context(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    expr: &HirExpr,
    state: &BodyState,
    allow_weak_upgrade_arg: bool,
    async_call_consumed: bool,
    live_after: &HashSet<String>,
) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            span,
            ..
        } => {
            check_async_call_consumed(analyzer, callee, resolution, span, async_call_consumed);
            check_constructor_field_initializers(analyzer, callee, args, expr, state);
            check_call_place_conflicts(analyzer, args, resolution, state);
            check_resource_pool_new_factory_contract(analyzer, callee, args);
            check_resource_pool_try_new_factory_contract(analyzer, callee, args);
            check_resource_pool_constructor_max_size_contract(analyzer, callee, args);
            check_resource_pool_factory_resource_captures(analyzer, callee, args, state);
            let weak_upgrade = is_weak_upgrade_callee(callee);
            let mut arg_live_after = live_after.clone();
            for arg in args.iter().rev() {
                if !tempdir_keep_consumes_resource_arg(callee, arg, state) {
                    check_expr_semantics_with_context(
                        analyzer,
                        local_analysis,
                        &arg.value,
                        state,
                        weak_upgrade,
                        false,
                        &arg_live_after,
                    );
                }
                collect_expr_uses(&arg.value, &mut arg_live_after);
            }
        }
        HirExpr::Spawn { value, .. } => {
            check_spawn_captures(analyzer, value, state);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                true,
                live_after,
            );
        }
        HirExpr::Await { value, .. } => {
            check_await_operand(analyzer, value, expr);
            let mut await_live_after = live_after.clone();
            collect_await_operand_live_uses(value, &mut await_live_after);
            check_await_live_values(analyzer, state, expr, &await_live_after);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                true,
                live_after,
            );
        }
        HirExpr::Effect {
            effect,
            value,
            span,
            ..
        } => {
            if matches!(effect, ParamEffect::Mut | ParamEffect::Take)
                && check_read_view_not_exclusive(analyzer, value, span, state)
            {
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    value,
                    state,
                    false,
                    async_call_consumed,
                    live_after,
                );
                return;
            }
            if matches!(effect, ParamEffect::Mut | ParamEffect::Take) && expr_is_fresh_shell(value)
            {
                fresh_requires_local_binding_diagnostic(analyzer, value, span);
            } else if *effect == ParamEffect::Take {
                check_take_operand_is_local(analyzer, value, span, state);
            } else if !(allow_weak_upgrade_arg && *effect == ParamEffect::Read)
                && matches!(effect, ParamEffect::Read | ParamEffect::Mut)
            {
                check_weak_field_requires_upgrade(analyzer, value);
            }
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                async_call_consumed,
                live_after,
            );
        }
        HirExpr::Try { value, .. } => {
            if let HirExpr::Try { span, .. } = expr {
                check_try_value_is_result(analyzer, value, span);
            }
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                async_call_consumed,
                live_after,
            );
        }
        HirExpr::Manage { value, span, .. } => {
            check_manage_operand_is_local(analyzer, value, span, state);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Binary { left, right, .. } => {
            let mut left_live_after = live_after.clone();
            collect_expr_uses(right, &mut left_live_after);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                left,
                state,
                false,
                false,
                &left_live_after,
            );
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                right,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Field { base, .. } => {
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                base,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Index { base, index, .. } => {
            let mut base_live_after = live_after.clone();
            collect_expr_uses(index, &mut base_live_after);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                base,
                state,
                false,
                false,
                &base_live_after,
            );
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                index,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Closure { body, .. } => {
            let mut closure_state = BodyState::default();
            check_block(
                analyzer,
                local_analysis,
                body,
                &mut closure_state,
                false,
                &HashSet::new(),
            );
        }
        HirExpr::Match {
            value,
            scrutinee_effect,
            arms,
            type_name,
            ..
        } => {
            check_match_scrutinee_type(analyzer, value);
            check_match_patterns_match_scrutinee(analyzer, value, arms);
            check_match_pattern_effects(analyzer, value, *scrutinee_effect, arms);
            if *scrutinee_effect == Some(DataEffect::Take) {
                check_take_operand_is_local(analyzer, value, &arm_span(arms), state);
            }
            check_match_expression_arm_types(analyzer, arms, type_name.as_deref());
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                async_call_consumed,
                live_after,
            );
            let base_state = state.clone();
            for arm in arms {
                let mut arm_state = base_state.clone();
                check_block(
                    analyzer,
                    local_analysis,
                    &arm.body,
                    &mut arm_state,
                    false,
                    live_after,
                );
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    &entry.key,
                    state,
                    allow_weak_upgrade_arg,
                    async_call_consumed,
                    live_after,
                );
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    &entry.value,
                    state,
                    allow_weak_upgrade_arg,
                    async_call_consumed,
                    live_after,
                );
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_match_expression_arm_types(
    analyzer: &mut Analyzer<'_>,
    arms: &[HirMatchArm],
    expected_type: Option<&str>,
) {
    let Some(expected_type) = expected_type else {
        return;
    };
    for arm in arms {
        let Some(arm_type) = match_arm_value_type(&arm.body) else {
            continue;
        };
        if arm_type == expected_type {
            continue;
        }
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::CONTROL_FLOW_TYPE_MISMATCH,
                format!(
                    "match arm has type `{arm_type}`, expected `{expected_type}` from the first produced arm."
                ),
                arm.span.clone(),
                "match arm type mismatch",
            )
            .with_cause("A match expression must produce one compatible value type across every arm.")
            .with_fix(
                "align_match_arm_types",
                "Return the same value type from every match expression arm.",
                "manual",
            ),
        );
    }
}

fn match_arm_value_type(block: &HirBlock) -> Option<&str> {
    match block.statements.iter().next_back()? {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => hir_expr_type_name(value),
        _ => None,
    }
}

fn check_await_placement(analyzer: &mut Analyzer<'_>, block: &HirBlock, function_is_async: bool) {
    // A `task_group` body is flattened into its parent block, recognizable by its
    // `async let` bindings; awaits of those handles are a structured-concurrency
    // boundary and are valid even in a synchronous enclosing function.
    let in_task_group = block
        .statements
        .iter()
        .any(|statement| matches!(statement, HirStmt::Let { is_async: true, .. }));
    let async_context = function_is_async || in_task_group;
    for statement in &block.statements {
        check_await_placement_stmt(analyzer, statement, async_context);
    }
}

fn check_await_placement_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    function_is_async: bool,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_await_placement_expr(analyzer, value, function_is_async);
            }
        }
        HirStmt::With { resource, body, .. } => {
            check_await_placement_expr(analyzer, resource, function_is_async);
            check_await_placement(analyzer, body, function_is_async);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_await_placement_expr(analyzer, condition, function_is_async);
            check_await_placement(analyzer, then_body, function_is_async);
            if let Some(else_body) = else_body {
                check_await_placement(analyzer, else_body, function_is_async);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_await_placement_expr(analyzer, condition, function_is_async);
            }
            check_await_placement(analyzer, body, function_is_async);
        }
        HirStmt::For { iterable, body, .. } => {
            check_await_placement_expr(analyzer, iterable, function_is_async);
            check_await_placement(analyzer, body, function_is_async);
        }
        HirStmt::Match { value, arms, .. } => {
            check_await_placement_expr(analyzer, value, function_is_async);
            for arm in arms {
                check_await_placement(analyzer, &arm.body, function_is_async);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                // The arm operation is `select`'s structured await boundary: an
                // executor poll loop drives it, so the await is valid even in a
                // synchronous enclosing function. The body is ordinary code.
                check_await_placement_expr(analyzer, &arm.operation, true);
                check_await_placement(analyzer, &arm.body, function_is_async);
            }
        }
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            check_await_placement_expr(analyzer, value, function_is_async)
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn check_await_placement_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    function_is_async: bool,
) {
    match expr {
        HirExpr::Await { value, span, .. } => {
            if !function_is_async {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::AWAIT_OUTSIDE_ASYNC,
                        "`await` is only valid inside an async function.",
                        span.clone(),
                        "await outside async fn",
                    )
                    .with_cause("Suspension points are part of the async function frame and cannot appear in ordinary synchronous functions.")
                    .with_fix("move_to_async_fn", "Move this await into an `async fn`, or call a synchronous API.", "manual"),
                );
            }
            check_await_placement_expr(analyzer, value, function_is_async);
        }
        HirExpr::Binary { left, right, .. } => {
            check_await_placement_expr(analyzer, left, function_is_async);
            check_await_placement_expr(analyzer, right, function_is_async);
        }
        HirExpr::Field { base, .. } => {
            check_await_placement_expr(analyzer, base, function_is_async)
        }
        HirExpr::Index { base, index, .. } => {
            check_await_placement_expr(analyzer, base, function_is_async);
            check_await_placement_expr(analyzer, index, function_is_async);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_await_placement_expr(analyzer, &arg.value, function_is_async);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Try { value, .. } => {
            check_await_placement_expr(analyzer, value, function_is_async);
        }
        HirExpr::Closure { body, .. } => check_await_placement(analyzer, body, false),
        HirExpr::Match { value, arms, .. } => {
            check_await_placement_expr(analyzer, value, function_is_async);
            for arm in arms {
                check_await_placement(analyzer, &arm.body, function_is_async);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_await_placement_expr(analyzer, &entry.key, function_is_async);
                check_await_placement_expr(analyzer, &entry.value, function_is_async);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_await_operand(analyzer: &mut Analyzer<'_>, value: &HirExpr, await_expr: &HirExpr) {
    if await_expr_targets_async_call(value) {
        return;
    }
    // Allow `await x` where x is an async let binding (task_group pending)
    if let Some(async_let_name) =
        await_targets_async_let_binding(value, &analyzer.async_let_names).map(str::to_string)
    {
        analyzer
            .async_let_names
            .retain(|name| name != &async_let_name);
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::AWAIT_NON_ASYNC,
            "`await` must consume an async call.",
            expr_span(await_expr).clone(),
            "await non-async expression",
        )
        .with_cause("RSScript does not expose Future or Task values in source; the executable async MVP only awaits direct async calls.")
        .with_fix("await_async_call", "Await an `async fn` call directly.", "manual"),
    );
}

fn await_targets_async_let_binding<'a>(
    expr: &'a HirExpr,
    async_let_names: &'a [String],
) -> Option<&'a str> {
    match expr {
        HirExpr::Ident { name, .. } if async_let_names.contains(name) => Some(name.as_str()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            await_targets_async_let_binding(value, async_let_names)
        }
        _ => None,
    }
}

fn await_expr_targets_async_call(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { resolution, .. } => {
            matches!(resolution, CallResolution::Resolved { signature, .. } if signature.is_async)
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            await_expr_targets_async_call(value)
        }
        _ => false,
    }
}

fn check_await_live_values(
    analyzer: &mut Analyzer<'_>,
    state: &BodyState,
    await_expr: &HirExpr,
    live_after: &HashSet<String>,
) {
    let span = expr_span(await_expr).clone();
    for resource in &state.resources {
        await_live_value_diagnostic(analyzer, "resource", resource, &span);
    }
    for local in &state.locals {
        if !live_after.contains(local) {
            continue;
        }
        if state
            .value_type(local)
            .is_some_and(|type_name| is_copy_type_name(type_name))
        {
            continue;
        }
        await_live_value_diagnostic(analyzer, "local value", local, &span);
    }
}

fn await_live_value_diagnostic(analyzer: &mut Analyzer<'_>, kind: &str, name: &str, span: &Span) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::AWAIT_LIVE_LOCAL,
            format!("{kind} `{name}` cannot live across `await`."),
            span.clone(),
            "value live across await",
        )
        .with_cause("Suspending an RSScript async frame may keep managed handles and Copy snapshots, but local values, resources, and runtime guards must not be retained across suspension.")
        .with_fix("drop_before_await", format!("End the lifetime of `{name}` before this `await`."), "manual"),
    );
}

fn expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

fn check_async_call_consumed(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    resolution: &CallResolution,
    span: &Span,
    async_call_consumed: bool,
) {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return;
    };
    if !signature.is_async || async_call_consumed {
        return;
    }

    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ASYNC_CALL_NOT_CONSUMED,
            format!(
                "async call `{}` must be awaited.",
                body_callee_display(callee)
            ),
            span.clone(),
            "async call must be awaited",
        )
        .with_cause(
            "Async calls introduce suspension boundaries that must be visible in source; `spawn` is reserved but not executable in v0.6.",
        )
        .with_fix(
            "await_async_call",
            format!(
                "Write `await {}(...)`.",
                body_callee_display(callee)
            ),
            "manual",
        ),
    );
}

fn is_weak_upgrade_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "Weak" && type_root_name(name) == "upgrade"
    )
}

fn check_weak_field_requires_upgrade(analyzer: &mut Analyzer<'_>, value: &HirExpr) {
    let Some(access) = weak_field_access_requiring_upgrade(value) else {
        return;
    };

    analyzer.diagnostics.push(
        Diagnostic::error(
            code::WEAK_FIELD_REQUIRES_UPGRADE,
            format!(
                "weak field `{}` must be upgraded before it is used as a value.",
                access.name
            ),
            access.span.clone(),
            "weak field requires upgrade",
        )
        .with_cause("A weak field is a non-owning handle and may no longer point to a live value.")
        .with_fix(
            "upgrade_weak_field",
            format!(
                "Use `Weak.upgrade(value: read {})` and handle `None`.",
                access.name
            ),
            "manual",
        ),
    );
}

fn expr_is_fresh_shell(expr: &HirExpr) -> bool {
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

fn fresh_requires_local_binding_diagnostic(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::FRESH_REQUIRES_LOCAL_BINDING,
            "`fresh` expression must be bound locally before `mut` or `take` use.",
            span.clone(),
            "fresh value requires local binding",
        )
        .with_cause("Direct fresh expressions can materialize as managed temporaries for `read`; `mut` and `take` require an explicit local owner.")
        .with_fix(
            "bind_fresh_local",
            format!(
                "Bind the value first, for example `local value = {}`.",
                hir_expr_hint(value)
            ),
            "manual",
        ),
    );
}

fn check_read_view_not_exclusive(
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

fn hir_expr_hint(expr: &HirExpr) -> String {
    match expr {
        HirExpr::Call { callee, .. } => body_callee_display(callee),
        HirExpr::Try { value, .. } => format!("{}?", hir_expr_hint(value)),
        HirExpr::Ident { name, .. } => name.clone(),
        _ => "fresh_expr".to_string(),
    }
}

fn body_callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!("{} {}.{method}", effect.as_str(), body_expr_label(receiver)),
    }
}

fn body_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Field { base, name, .. } => format!("{}.{}", body_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", body_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", body_callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            body_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

fn weak_field_access_requiring_upgrade(expr: &HirExpr) -> Option<&crate::hir::HirFieldAccess> {
    match expr {
        HirExpr::Field { base, access, .. } => {
            if access.is_weak {
                Some(access)
            } else {
                weak_field_access_requiring_upgrade(base)
            }
        }
        HirExpr::Call { callee, args, .. } if is_weak_upgrade_callee(callee) => {
            for arg in args {
                if let HirExpr::Effect { value, .. } = &arg.value
                    && weak_field_access_requiring_upgrade(value).is_some()
                {
                    return None;
                }
            }
            args.iter()
                .find_map(|arg| weak_field_access_requiring_upgrade(&arg.value))
        }
        HirExpr::Call { args, .. } => args
            .iter()
            .find_map(|arg| weak_field_access_requiring_upgrade(&arg.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Try { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => weak_field_access_requiring_upgrade(value),
        HirExpr::Index { base, index, .. } => weak_field_access_requiring_upgrade(base)
            .or_else(|| weak_field_access_requiring_upgrade(index)),
        HirExpr::Binary { left, right, .. } => weak_field_access_requiring_upgrade(left)
            .or_else(|| weak_field_access_requiring_upgrade(right)),
        HirExpr::Closure { body, .. } => body
            .statements
            .iter()
            .find_map(|statement| weak_field_access_requiring_upgrade_in_stmt(statement)),
        HirExpr::Match { value, arms, .. } => {
            weak_field_access_requiring_upgrade(value).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body
                        .statements
                        .iter()
                        .find_map(weak_field_access_requiring_upgrade_in_stmt)
                })
            })
        }
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| weak_field_access_requiring_upgrade(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| weak_field_access_requiring_upgrade(&entry.value))
            }),
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn weak_field_access_requiring_upgrade_in_stmt(
    statement: &HirStmt,
) -> Option<&crate::hir::HirFieldAccess> {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => weak_field_access_requiring_upgrade(value),
        HirStmt::With { resource, body, .. } => weak_field_access_requiring_upgrade(resource)
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(weak_field_access_requiring_upgrade_in_stmt)
            }),
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => weak_field_access_requiring_upgrade(condition)
            .or_else(|| {
                then_body
                    .statements
                    .iter()
                    .find_map(weak_field_access_requiring_upgrade_in_stmt)
            })
            .or_else(|| {
                else_body.as_ref().and_then(|body| {
                    body.statements
                        .iter()
                        .find_map(weak_field_access_requiring_upgrade_in_stmt)
                })
            }),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(weak_field_access_requiring_upgrade)
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(weak_field_access_requiring_upgrade_in_stmt)
            }),
        HirStmt::For { iterable, body, .. } => weak_field_access_requiring_upgrade(iterable)
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(weak_field_access_requiring_upgrade_in_stmt)
            }),
        HirStmt::Match { value, arms, .. } => {
            weak_field_access_requiring_upgrade(value).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body
                        .statements
                        .iter()
                        .find_map(weak_field_access_requiring_upgrade_in_stmt)
                })
            })
        }
        HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
            weak_field_access_requiring_upgrade(&arm.operation).or_else(|| {
                arm.body
                    .statements
                    .iter()
                    .find_map(weak_field_access_requiring_upgrade_in_stmt)
            })
        }),
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn check_constructor_field_initializers(
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
                Diagnostic::error(
                    code::WEAK_FIELD_REQUIRES_WEAK_HANDLE,
                    format!(
                        "weak field `{name}` for `{constructor_name}` must be initialized from an explicit weak handle."
                    ),
                    hir_expr_span(&arg.value).clone(),
                    "weak field requires weak handle",
                )
                .with_cause(
                    "Weak fields are non-owning handles. Initializing them must be syntax-visible.",
                )
                .with_fix(
                    "wrap_with_weak_from",
                    format!("Write `{name}: Weak.from(value: read target)` in the constructor."),
                    "manual",
                ),
            );
        } else if field.is_handle && actual_effect != Some("read") {
            constructor_field_effect_diagnostic(
                analyzer,
                &constructor_name,
                name,
                "read",
                &arg.value,
                "Handle fields store managed handles and must be initialized from an explicit `read` value.",
            );
        } else if !field.is_handle
            && constructor_arg_uses_local_inline_place(&arg.value, state)
            && actual_effect != Some("take")
        {
            constructor_field_effect_diagnostic(
                analyzer,
                &constructor_name,
                name,
                "take",
                &arg.value,
                "Inline fields take ownership of non-Copy local values stored inside the constructed struct.",
            );
        } else if !field.is_handle
            && analyzer.hir.type_kind(&field.type_name).is_some()
            && !is_copy_type_name(&field.type_name)
            && constructor_arg_uses_managed_inline_value(analyzer, &arg.value, state)
        {
            managed_inline_constructor_field_diagnostic(
                analyzer,
                &constructor_name,
                name,
                &arg.value,
            );
        }
    }
}

fn constructor_field_arg_name<'a>(
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

fn constructor_arg_uses_local_inline_place(expr: &HirExpr, state: &BodyState) -> bool {
    let Some(path) = constructor_arg_place_path(expr) else {
        return false;
    };
    state.is_local(&path.base) && !path.crosses_handle
}

fn constructor_arg_place_path(expr: &HirExpr) -> Option<PlacePath> {
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
        | HirExpr::Unknown(_) => None,
    }
}

fn constructor_arg_uses_managed_inline_value(
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
                signature.return_type.as_deref().is_some_and(|type_name| {
                    let type_name = type_name
                        .trim()
                        .strip_prefix("fresh ")
                        .unwrap_or(type_name.trim());
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
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Unknown(_) => false,
    }
}

fn is_weak_handle_producing_expr(expr: &HirExpr) -> bool {
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

fn expr_data_effect(expr: &HirExpr) -> Option<&'static str> {
    match expr {
        HirExpr::Effect { effect, .. } => Some(effect.as_str()),
        _ => None,
    }
}

fn constructor_field_effect_diagnostic(
    analyzer: &mut Analyzer<'_>,
    constructor_name: &str,
    field_name: &str,
    expected: &str,
    value: &HirExpr,
    cause: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::MISSING_DATA_EFFECT,
            format!(
                "field `{field_name}` for `{constructor_name}` must be initialized with `{expected}`."
            ),
            hir_expr_span(value).clone(),
            "missing constructor field effect",
        )
        .with_cause(cause)
        .with_fix(
            "add_constructor_field_effect",
            format!("Write `{field_name}: {expected} ...` in the constructor."),
            "machine-applicable",
        ),
    );
}

fn managed_inline_constructor_field_diagnostic(
    analyzer: &mut Analyzer<'_>,
    constructor_name: &str,
    field_name: &str,
    value: &HirExpr,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::MISSING_DATA_EFFECT,
            format!(
                "field `{field_name}` for `{constructor_name}` cannot be initialized from a managed value."
            ),
            hir_expr_span(value).clone(),
            "managed value used for inline field",
        )
        .with_cause(
            "Inline non-Copy fields own their stored value. RSScript has no implicit clone from managed values into inline fields.",
        )
        .with_fix(
            "make_field_handle_or_bind_local",
            "Use a `handle` field, construct a fresh inline value, or bind the value as `local` and pass it with `take`.",
            "manual",
        ),
    );
}

fn check_spawn_captures(analyzer: &mut Analyzer<'_>, value: &HirExpr, state: &BodyState) {
    let mut captures = Vec::new();
    collect_spawn_capture_idents(value, &mut captures);
    for (name, span) in captures {
        if state.is_local(&name) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::LOCAL_VALUE_RETAINED,
                    format!("spawn cannot capture local value `{name}`."),
                    span,
                    "local captured by spawn",
                )
                .with_cause("`spawn` may retain captured values until task completion.")
                .with_fix(
                    "manage_before_spawn",
                    format!("Convert `{name}` through `manage` before spawning the task."),
                    "manual",
                ),
            );
        } else if state.is_resource(&name) {
            resource_escape_diagnostic(analyzer, &name, span);
        }
    }
}

fn collect_spawn_capture_idents(expr: &HirExpr, captures: &mut Vec<(String, Span)>) {
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
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_spawn_capture_idents_from_stmt(statement: &HirStmt, captures: &mut Vec<(String, Span)>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => collect_spawn_capture_idents(value, captures),
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

fn check_call_place_conflicts(
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
            .filter(|param| param.type_name.starts_with("noescape"))
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
    // is a local-only capability. `local` bindings and `take` parameters are
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
fn collect_closure_capture_accesses(body: &HirBlock, out: &mut Vec<CallPlaceAccess>) {
    let mut bound = HashSet::new();
    collect_closure_bound_names(body, &mut bound);
    collect_closure_effect_accesses_block(body, &bound, out);
}

fn check_noescape_consuming_captured_locals(
    analyzer: &mut Analyzer<'_>,
    accesses: &[CallPlaceAccess],
    state: &BodyState,
) {
    for access in accesses {
        if access.moves_path && state.is_local(&access.path.base) {
            noescape_consumes_capture_diagnostic(analyzer, access);
        }
    }
}

fn collect_closure_bound_names(block: &HirBlock, bound: &mut HashSet<String>) {
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

fn collect_closure_effect_accesses_block(
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

fn collect_closure_effect_accesses_expr(
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
                collect_closure_effect_accesses_block(&arm.body, bound, out);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_closure_effect_accesses_expr(&entry.key, bound, out);
                collect_closure_effect_accesses_expr(&entry.value, bound, out);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn effect_of(expr: &HirExpr) -> ParamEffect {
    match expr {
        HirExpr::Effect { effect, .. } => *effect,
        _ => ParamEffect::Read,
    }
}

fn call_place_access(arg: &HirCallArg) -> Option<CallPlaceAccess> {
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

fn expr_moves_path(expr: &HirExpr) -> bool {
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
        | HirExpr::Unknown(_) => false,
    }
}

fn check_try_value_is_result(analyzer: &mut Analyzer<'_>, value: &HirExpr, span: &Span) {
    let Some(type_name) = hir_expr_type_name(value) else {
        return;
    };
    if is_result_type(type_name) {
        return;
    }

    analyzer.diagnostics.push(
        Diagnostic::error(
            code::INVALID_TRY_OPERATOR,
            "`?` can only be applied to a Result value.",
            span.clone(),
            "invalid try operator",
        )
        .with_cause(format!(
            "The expression before `?` has type `{type_name}`, not `Result<T, E>`."
        ))
        .with_fix(
            "remove_try_or_return_result",
            "Remove `?`, or call an API that returns `Result<T, E>`.",
            "manual",
        ),
    );
}

fn check_try_error_types(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    function_error_type: Option<&str>,
) {
    for statement in &block.statements {
        check_try_error_types_stmt(analyzer, statement, function_error_type);
    }
}

fn check_try_error_types_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    function_error_type: Option<&str>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type)
        }
        HirStmt::With { resource, body, .. } => {
            check_try_error_types_expr(analyzer, resource, function_error_type);
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_try_error_types_expr(analyzer, condition, function_error_type);
            check_try_error_types(analyzer, then_body, function_error_type);
            if let Some(else_body) = else_body {
                check_try_error_types(analyzer, else_body, function_error_type);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_try_error_types_expr(analyzer, condition, function_error_type);
            }
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirStmt::For { iterable, body, .. } => {
            check_try_error_types_expr(analyzer, iterable, function_error_type);
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirStmt::Match { value, arms, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type);
            for arm in arms {
                check_try_error_types(analyzer, &arm.body, function_error_type);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_try_error_types_expr(analyzer, &arm.operation, function_error_type);
                check_try_error_types(analyzer, &arm.body, function_error_type);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn check_try_error_types_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    function_error_type: Option<&str>,
) {
    match expr {
        HirExpr::Try { value, span, .. } => {
            if let (Some(function_error_type), Some(operand_type)) =
                (function_error_type, hir_expr_type_name(value))
                && let Some(operand_error_type) = result_error_type_name(operand_type)
                && operand_error_type != function_error_type
            {
                try_error_type_mismatch_diagnostic(
                    analyzer,
                    span,
                    operand_error_type,
                    function_error_type,
                );
            }
            check_try_error_types_expr(analyzer, value, function_error_type);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_try_error_types_expr(analyzer, &arg.value, function_error_type);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type);
        }
        HirExpr::Binary { left, right, .. } => {
            check_try_error_types_expr(analyzer, left, function_error_type);
            check_try_error_types_expr(analyzer, right, function_error_type);
        }
        HirExpr::Field { base, .. } => {
            check_try_error_types_expr(analyzer, base, function_error_type);
        }
        HirExpr::Index { base, index, .. } => {
            check_try_error_types_expr(analyzer, base, function_error_type);
            check_try_error_types_expr(analyzer, index, function_error_type);
        }
        HirExpr::Closure { body, .. } => {
            check_try_error_types(analyzer, body, function_error_type);
        }
        HirExpr::Match { value, arms, .. } => {
            check_try_error_types_expr(analyzer, value, function_error_type);
            for arm in arms {
                check_try_error_types(analyzer, &arm.body, function_error_type);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_try_error_types_expr(analyzer, &entry.key, function_error_type);
                check_try_error_types_expr(analyzer, &entry.value, function_error_type);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| builtin_value_type_name(name)),
        HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn builtin_value_type_name(name: &str) -> Option<&'static str> {
    match name {
        "true" | "false" => Some("Bool"),
        "null" => Some("JsonLiteral"),
        "Unit" => Some("Unit"),
        "None" => Some("Option<?>"),
        _ => None,
    }
}

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

fn is_result_type(type_name: &str) -> bool {
    type_name == "Result" || type_name.starts_with("Result<")
}

fn result_error_type_ref_name(return_ty: &TypeRef) -> Option<String> {
    if return_ty.name != "Result" || return_ty.args.len() != 2 {
        return None;
    }
    return_ty.args.get(1).map(type_ref_name)
}

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

fn place_path(expr: &HirExpr) -> Option<PlacePath> {
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

fn check_place_pair_conflict(
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
fn base_allows_field_split(analyzer: &Analyzer<'_>, state: &BodyState, base: &str) -> bool {
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

fn managed_field_split_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::MANAGED_FIELD_SPLIT_CONFLICT,
            format!(
                "managed object fields `{}` and `{}` cannot be split in one call.",
                place_path_display(&left.path),
                place_path_display(&right.path)
            ),
            right.span.clone(),
            "managed field split conflict",
        )
        .with_cause(
            "Field splitting into disjoint inline paths is a local-only capability. A managed object is a single runtime value behind one write guard, so two mutable accesses to its inline fields conflict; the conflict root is the managed object base.",
        )
        .with_fix(
            "split_managed_field_accesses",
            "Split the accesses into separate statements, or move the fields behind explicit `handle` fields so they become distinct managed objects.",
            "manual",
        ),
    );
}

fn move_base_field_conflict(left: &CallPlaceAccess, right: &CallPlaceAccess) -> bool {
    (left.moves_path && move_path_conflicts_with_access(&left.path, &right.path))
        || (right.moves_path && move_path_conflicts_with_access(&right.path, &left.path))
}

fn move_path_conflicts_with_access(moved: &PlacePath, accessed: &PlacePath) -> bool {
    if moved.components.is_empty() || accessed.components.is_empty() {
        return true;
    }

    moved.has_index
        || accessed.has_index
        || moved.crosses_handle
        || accessed.crosses_handle
        || path_prefix_or_equal(&moved.components, &accessed.components)
}

fn pair_mutates(left: &CallPlaceAccess, right: &CallPlaceAccess) -> bool {
    mutates(left.effect) || mutates(right.effect)
}

fn mutates(effect: ParamEffect) -> bool {
    matches!(effect, ParamEffect::Mut | ParamEffect::Take)
}

fn whole_base_or_prefix_access(left: &PlacePath, right: &PlacePath) -> bool {
    left.components.is_empty() != right.components.is_empty()
        && (is_prefix(&left.components, &right.components)
            || is_prefix(&right.components, &left.components))
}

fn path_prefix_or_equal(left: &[String], right: &[String]) -> bool {
    is_prefix(left, right) || is_prefix(right, left)
}

fn is_prefix(left: &[String], right: &[String]) -> bool {
    left.len() <= right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| left == right)
}

fn place_path_display(path: &PlacePath) -> String {
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

fn field_partial_access_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::FIELD_PARTIAL_ACCESS_CONFLICT,
            format!(
                "call mixes whole local access `{}` with field access `{}`.",
                place_path_display(&left.path),
                place_path_display(&right.path)
            ),
            right.span.clone(),
            "whole-base field conflict",
        )
        .with_cause("A whole local base or prefix conflicts with a mutable or taking subpath in the same call.")
        .with_fix(
            "split_call",
            "Split the whole-base read and field mutation into separate statements or pass disjoint fields explicitly.",
            "manual",
        ),
    );
}

fn field_prefix_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
    cause: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::FIELD_PREFIX_CONFLICT,
            format!(
                "local field paths `{}` and `{}` are not disjoint.",
                place_path_display(&left.path),
                place_path_display(&right.path)
            ),
            right.span.clone(),
            "field path conflict",
        )
        .with_cause(cause)
        .with_fix(
            "split_or_refactor_paths",
            "Split the accesses into separate calls or refactor through explicit split APIs.",
            "manual",
        ),
    );
}

fn indexed_place_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::INDEXED_PARTIAL_ACCESS_CONFLICT,
            format!(
                "indexed local paths `{}` and `{}` cannot be proven disjoint.",
                place_path_display(&left.path),
                place_path_display(&right.path)
            ),
            right.span.clone(),
            "indexed local access conflict",
        )
        .with_cause("RSScript v0.6 treats indexed access as access to the whole local container for alias checking.")
        .with_fix(
            "use_split_api",
            "Use an explicit container split API that proves or checks disjoint element access.",
            "manual",
        ),
    );
}

fn move_base_field_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    let (moved, accessed) = if left.moves_path {
        (left, right)
    } else {
        (right, left)
    };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::MOVE_BASE_FIELD_CONFLICT,
            format!(
                "call moves local path `{}` while also accessing `{}`.",
                place_path_display(&moved.path),
                place_path_display(&accessed.path)
            ),
            moved.span.clone(),
            "move-base field conflict",
        )
        .with_cause("A local base cannot be `manage`d or `take`n in the same expression where one of its fields is accessed.")
        .with_fix(
            "split_move_from_field_access",
            "Split the field access and `manage`/`take` into separate statements.",
            "manual",
        ),
    );
}

fn check_manage_operand_is_local(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) {
    let Some(name) = hir_ident_name(value) else {
        invalid_manage_operand_diagnostic(
            analyzer,
            "`manage` can only move a named local binding.",
            span.clone(),
        );
        return;
    };
    if !state.is_local(name) {
        if state.is_read_view(name) {
            read_view_mutation_diagnostic(analyzer, name, span.clone());
            return;
        }
        invalid_manage_operand_diagnostic(
            analyzer,
            format!("`{name}` is not a local binding and cannot be moved with `manage`."),
            span.clone(),
        );
    }
}

fn read_view_mutation_diagnostic(analyzer: &mut Analyzer<'_>, name: &str, span: Span) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::READ_VIEW_MUTATION,
            format!("`{name}` is a read view from a `for` loop and cannot be used as an exclusive value."),
            span,
            "read view mutation",
        )
        .with_cause("RSScript `for` iterates `List<T>` by read view for non-Copy struct elements, so the loop variable does not own the element.")
        .with_fix(
            "copy_before_mutating",
            "Create a fresh local copy before mutation, or use an explicit partitioning API that grants exclusive element ownership.",
            "manual",
        ),
    );
}

fn check_take_operand_is_local(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) {
    let Some(path) = place_path(value) else {
        invalid_take_operand_diagnostic(
            analyzer,
            "`take` can only consume a named local binding or a local field path.",
            span.clone(),
        );
        return;
    };
    if !state.is_local(&path.base) {
        if state.is_resource(&path.base) {
            resource_escape_diagnostic(analyzer, &path.base, span.clone());
            return;
        }
        invalid_take_operand_diagnostic(
            analyzer,
            format!(
                "`{}` is not a local binding and cannot be consumed with `take`.",
                path.base
            ),
            span.clone(),
        );
    }
}

fn tempdir_keep_consumes_resource_arg(
    callee: &Callee,
    arg: &HirCallArg,
    state: &BodyState,
) -> bool {
    let is_tempdir_keep = matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "TempDir" && name == "keep"
    ) || matches!(callee, Callee::Name(name) if name == "TempDir.keep");
    if !is_tempdir_keep || arg.name.as_deref().unwrap_or("dir") != "dir" {
        return false;
    }
    let HirExpr::Effect {
        effect: ParamEffect::Take,
        value,
        ..
    } = &arg.value
    else {
        return false;
    };
    matches!(value.as_ref(), HirExpr::Ident { name, .. } if state.is_resource(name))
}

fn hir_ident_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

fn apply_expr_effects(expr: &HirExpr, state: &mut BodyState) {
    match expr {
        HirExpr::Call { args, events, .. } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            for arg in args {
                apply_expr_effects(&arg.value, state);
            }
        }
        HirExpr::Effect { value, events, .. } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            apply_expr_effects(value, state);
        }
        HirExpr::Manage {
            value,
            events,
            span,
            ..
        } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            if let Some(path) = place_path(value) {
                state.mark_moved(&place_path_display(&path), span.clone());
            }
            apply_expr_effects(value, state);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            apply_expr_effects(value, state)
        }
        HirExpr::Try { value, .. } => apply_expr_effects(value, state),
        HirExpr::Match {
            value,
            scrutinee_effect,
            span,
            ..
        } => {
            apply_expr_effects(value, state);
            apply_match_scrutinee_effect(*scrutinee_effect, value, span, state);
        }
        HirExpr::Binary { left, right, .. } => {
            apply_expr_effects(left, state);
            apply_expr_effects(right, state);
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                apply_expr_effects(&entry.key, state);
                apply_expr_effects(&entry.value, state);
            }
        }
        HirExpr::Field { base, .. } => apply_expr_effects(base, state),
        HirExpr::Index { base, index, .. } => {
            apply_expr_effects(base, state);
            apply_expr_effects(index, state);
        }
        HirExpr::Closure { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn apply_match_scrutinee_effect(
    effect: Option<DataEffect>,
    value: &HirExpr,
    span: &Span,
    state: &mut BodyState,
) {
    if effect != Some(DataEffect::Take) {
        return;
    }
    if let Some(path) = place_path(value) {
        state.mark_moved(&place_path_display(&path), span.clone());
    }
}

fn arm_span(arms: &[HirMatchArm]) -> Span {
    arms.first().map_or_else(
        || Span {
            file: String::new(),
            line: 1,
            column: 1,
            length: 1,
        },
        |arm| arm.span.clone(),
    )
}

fn check_moved_uses(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis) {
    for moved_use in local_analysis.moved_uses() {
        moved_use_diagnostic(analyzer, moved_use);
    }
}

fn check_managed_to_local_uses(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis) {
    for managed_to_local in local_analysis.managed_to_local_uses() {
        managed_to_local_diagnostic(analyzer, managed_to_local);
    }
}

fn check_retained_local_uses(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis) {
    for retained in local_analysis.retained_local_uses() {
        retained_local_diagnostic(analyzer, retained);
    }
}

fn check_retained_closure_captures(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis) {
    for capture in local_analysis.retained_closure_captures() {
        retained_closure_capture_diagnostic(analyzer, capture);
    }
}

fn check_take_handle_fields(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis) {
    for field in local_analysis.take_handle_fields() {
        take_handle_field_diagnostic(analyzer, field);
    }
}

fn check_fresh_returns(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    function: &FunctionDecl,
) {
    if !function.returns_fresh {
        return;
    }
    check_fresh_return_type(analyzer, function);
    for issue in local_analysis.fresh_return_issues() {
        match &issue.kind {
            FreshReturnIssueKind::NotClean { name } => {
                fresh_return_diagnostic(analyzer, &function.name, name, issue.span);
            }
            FreshReturnIssueKind::UnknownIdent { name } if trusted_fresh_ident(analyzer, name) => {}
            FreshReturnIssueKind::UnknownIdent { .. } | FreshReturnIssueKind::Unknown => {
                freshness_unknown_diagnostic(analyzer, &function.name, issue);
            }
        }
    }
}

fn check_fresh_return_type(analyzer: &mut Analyzer<'_>, function: &FunctionDecl) {
    let Some(return_ty) = &function.return_ty else {
        return;
    };
    let target = fresh_return_target_type(return_ty);
    match analyzer.hir.type_kind(&target.name) {
        // `fresh` is a shallow newly-created-shell guarantee: valid for struct,
        // sum, and class constructors (a freshly created class instance). Only a
        // `resource` (an opaque managed handle) can't be `fresh`.
        Some(HirTypeKind::Struct | HirTypeKind::Sum | HirTypeKind::Class) | None => {}
        Some(HirTypeKind::Resource) => {
            invalid_fresh_return_type_diagnostic(analyzer, function, target);
        }
    }
}

fn fresh_return_target_type(return_ty: &TypeRef) -> &TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}

fn managed_to_local_diagnostic(analyzer: &mut Analyzer<'_>, managed_to_local: ManagedToLocalUse) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::MANAGED_TO_LOCAL,
            format!(
                "managed value cannot be converted to local binding `{}`.",
                managed_to_local.local_name
            ),
            managed_to_local.span,
            "managed value used as local",
        )
        .with_cause(format!(
            "`{}` is already managed; RSScript has no managed -> local conversion.",
            managed_to_local.managed_name
        ))
        .with_fix(
            "create_local",
            "Create the value as `local` at its creation point.",
            "manual",
        ),
    );
}

fn take_handle_field_diagnostic(analyzer: &mut Analyzer<'_>, field: &TakeHandleField) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::TAKE_HANDLE_FIELD,
            format!("cannot `take` handle field `{}`.", field.name),
            field.span.clone(),
            "take of handle field",
        )
        .with_cause(
            "Handle fields are managed references and cannot be consumed as local inline values.",
        ),
    );
}

fn retained_local_diagnostic(analyzer: &mut Analyzer<'_>, retained: RetainedLocalUse) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::LOCAL_VALUE_RETAINED,
            format!(
                "retaining API `{}` cannot retain local value `{}`.",
                retained.callee, retained.name
            ),
            retained.span,
            "local value retained",
        )
        .with_cause(format!(
            "`{}` declares `effects(retains({}))`.",
            retained.callee, retained.param
        ))
        .with_fix(
            "manage_local",
            format!(
                "Pass `{}` through `manage {}` before retaining it.",
                retained.param, retained.name
            ),
            "manual",
        ),
    );
}

fn retained_closure_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    capture: RetainedClosureCapture,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
            format!(
                "retained closure passed to `{}` captures local value `{}`.",
                capture.callee, capture.name
            ),
            capture.capture_span,
            "local captured here",
        )
        .with_cause(format!(
            "`{}` declares `effects(retains({}))`; the closure may outlive local values.",
            capture.callee, capture.param
        ))
        .with_cause(format!(
            "The retained closure starts at {}:{}.",
            capture.closure_span.line, capture.closure_span.column
        ))
        .with_fix(
            "avoid_retained_capture",
            "Do not capture local values in closures passed to retaining APIs.",
            "manual",
        ),
    );
}

fn noescape_consumes_capture_diagnostic(analyzer: &mut Analyzer<'_>, access: &CallPlaceAccess) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::NOESCAPE_CONSUMES_CAPTURE,
            format!(
                "noescape closure cannot consume captured local value `{}`.",
                access.path.base
            ),
            access.span.clone(),
            "captured local consumed here",
        )
        .with_cause(
            "`noescape Fn()` callbacks are non-consuming; the callee may call this closure more than once.",
        )
        .with_cause(
            "Read or mutate the captured local inside the noescape closure, or move/manage it before constructing the closure.",
        )
        .with_fix(
            "avoid_consuming_capture",
            "Do not use `take` or `manage` on captured local values inside noescape callbacks.",
            "manual",
        ),
    );
}

fn explicit_closure_missing_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    actual: ParamEffect,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CLOSURE_CAPTURE_CONTRACT,
            format!("closure uses `{name}` without declaring it in captures"),
            span,
            "missing closure capture",
        )
        .with_cause(
            "Escaping function values must make every external input explicit in `captures(...)`.",
        )
        .with_cause(format!(
            "Add `captures({} {name})` or remove the external use.",
            actual.as_str()
        )),
    );
}

fn explicit_closure_unused_capture_diagnostic(analyzer: &mut Analyzer<'_>, name: &str, span: Span) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CLOSURE_CAPTURE_CONTRACT,
            format!("closure declares capture `{name}` but does not use it"),
            span,
            "unused closure capture",
        )
        .with_cause(
            "A closure capture list is review evidence and must describe the function value's real inputs.",
        )
        .with_cause("Remove the capture entry or use the value inside the closure body."),
    );
}

fn explicit_closure_capture_contract_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    declared: ParamEffect,
    actual: ParamEffect,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CLOSURE_CAPTURE_CONTRACT,
            format!(
                "closure capture `{name}` is declared as `{}` but used as `{}`",
                declared.as_str(),
                actual.as_str()
            ),
            span,
            "closure capture effect mismatch",
        )
        .with_cause("Closure captures use the same read/mut/take ownership vocabulary as parameters.")
        .with_cause(format!(
            "Change the capture to `{} {name}` or change the closure body to match the declared access.",
            actual.as_str()
        )),
    );
}

fn try_error_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: &Span,
    operand_error_type: &str,
    function_error_type: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::INVALID_TRY_OPERATOR,
            "`?` error type must exactly match the function error type.",
            span.clone(),
            "mismatched try error type",
        )
        .with_cause(format!(
            "The operand returns `Result<_, {operand_error_type}>`, but the function returns `Result<_, {function_error_type}>`."
        ))
        .with_cause("RSScript does not perform implicit error conversion for `?`.")
        .with_fix(
            "map_error_explicitly",
            "Handle the error explicitly and return the function's error type.",
            "manual",
        ),
    );
}

fn moved_use_diagnostic(analyzer: &mut Analyzer<'_>, moved_use: MovedUse) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::USE_AFTER_MANAGE,
            format!(
                "`{}` was moved into the managed runtime by `manage {}`.",
                moved_use.name, moved_use.name
            ),
            moved_use.use_span,
            "used after manage",
        )
        .with_cause(format!(
            "The move happened at {}:{}.",
            moved_use.move_span.line, moved_use.move_span.column
        ))
        .with_fix(
            "move_use_before_manage",
            format!("Move this use before `manage {}`.", moved_use.name),
            "manual",
        ),
    );
}

fn check_managed_closure_captures(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    statement_span: &crate::diagnostic::Span,
    state: &BodyState,
) {
    let uses = local_analysis
        .managed_closure_ident_uses(statement_span)
        .unwrap_or(&[]);
    for (name, span) in uses {
        if state.is_local(name) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
                    format!("managed closure captures local value `{name}`."),
                    span.clone(),
                    "local captured here",
                )
                .with_cause("Closures bound with `let` are managed closures.")
                .with_fix(
                    "use_local_closure",
                    "Bind the closure with `local` or use a noescape callback.",
                    "manual",
                ),
            );
        }
    }
}

fn check_resource_escape(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    with_span: &crate::diagnostic::Span,
) {
    if let Some(escapes) = local_analysis.resource_escapes(with_span) {
        for escape in escapes {
            if !resource_is_active_at(local_analysis, &escape.binding, &escape.span) {
                continue;
            }
            match escape.kind {
                ResourceEscapeKind::Escape => {
                    resource_escape_diagnostic(analyzer, &escape.binding, escape.span.clone());
                }
                ResourceEscapeKind::Capture => {
                    resource_capture_diagnostic(analyzer, &escape.binding, escape.span.clone());
                }
            }
        }
    }
}

fn check_resource_pool_lease_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    within_with_resource: bool,
) {
    match expr {
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if !within_with_resource && is_resource_pool_lease_producer(callee) {
                resource_pool_lease_escape_diagnostic(analyzer, span.clone());
            }
            for arg in args {
                check_resource_pool_lease_expr(analyzer, &arg.value, false);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_pool_lease_expr(analyzer, value, within_with_resource);
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_pool_lease_expr(analyzer, left, within_with_resource);
            check_resource_pool_lease_expr(analyzer, right, within_with_resource);
        }
        HirExpr::Field { base, .. } => {
            check_resource_pool_lease_expr(analyzer, base, within_with_resource);
        }
        HirExpr::Index { base, index, .. } => {
            check_resource_pool_lease_expr(analyzer, base, within_with_resource);
            check_resource_pool_lease_expr(analyzer, index, within_with_resource);
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_pool_lease_expr(analyzer, value, within_with_resource);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_pool_lease_expr(analyzer, &entry.key, within_with_resource);
                check_resource_pool_lease_expr(analyzer, &entry.value, within_with_resource);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_resource_producer_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    allowed_resource_context: bool,
) {
    if expr_is_resource_producer(analyzer, expr) {
        if allowed_resource_context {
            check_resource_producer_children(analyzer, expr);
        } else {
            resource_producer_escape_diagnostic(
                analyzer,
                hir_expr_span(expr).clone(),
                hir_expr_type_name(expr).unwrap_or("resource"),
            );
        }
        return;
    }

    match expr {
        HirExpr::Call { callee, args, .. } => {
            for arg in args {
                if is_resource_pool_constructor(callee) && arg.name.as_deref() == Some("create") {
                    check_resource_pool_factory_expr(analyzer, &arg.value);
                } else {
                    check_resource_producer_expr(analyzer, &arg.value, false);
                }
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_producer_expr(analyzer, value, allowed_resource_context);
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_producer_expr(analyzer, left, false);
            check_resource_producer_expr(analyzer, right, false);
        }
        HirExpr::Field { base, .. } => check_resource_producer_expr(analyzer, base, false),
        HirExpr::Index { base, index, .. } => {
            check_resource_producer_expr(analyzer, base, false);
            check_resource_producer_expr(analyzer, index, false);
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_producer_expr(analyzer, value, allowed_resource_context);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_producer_expr(analyzer, &entry.key, false);
                check_resource_producer_expr(analyzer, &entry.value, false);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_result_resource_with_has_try(analyzer: &mut Analyzer<'_>, resource: &HirExpr) {
    if matches!(resource, HirExpr::Try { .. }) {
        return;
    }
    let Some(resource_type) = result_resource_ok_type(analyzer, resource) else {
        return;
    };

    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_PRODUCER_MISSING_TRY,
            format!(
                "`with` over `Result<{resource_type}, E>` must explicitly unwrap the resource producer with `?`."
            ),
            hir_expr_span(resource).clone(),
            "missing resource producer `?`",
        )
        .with_cause("Resource-producing `Result` values are transient; the successful resource must enter the `with` scope explicitly.")
        .with_fix(
            "add_try_to_resource_producer",
            "Write `with producer(...)? as resource { ... }`.",
            "machine-applicable",
        ),
    );
}

fn check_resource_producer_children(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    match expr {
        HirExpr::Call { callee, args, .. } => {
            for arg in args {
                if is_resource_pool_constructor(callee) && arg.name.as_deref() == Some("create") {
                    check_resource_pool_factory_expr(analyzer, &arg.value);
                } else {
                    check_resource_producer_expr(analyzer, &arg.value, false);
                }
            }
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            check_resource_producer_expr(analyzer, value, true);
        }
        _ => {}
    }
}

fn check_resource_producer_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => check_resource_producer_expr(analyzer, value, false),
        HirStmt::With { resource, body, .. } => {
            check_resource_producer_expr(analyzer, resource, true);
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_producer_expr(analyzer, condition, false);
            for statement in &then_body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_producer_expr(analyzer, condition, false);
            }
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_producer_expr(analyzer, iterable, false);
            for statement in &body.statements {
                check_resource_producer_stmt(analyzer, statement);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_producer_expr(analyzer, value, false);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_producer_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_producer_expr(analyzer, &arm.operation, false);
                for statement in &arm.body.statements {
                    check_resource_producer_stmt(analyzer, statement);
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

fn check_resource_pool_factory_expr(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    match expr {
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        _ => check_resource_producer_expr(analyzer, expr, true),
    }
}

fn check_resource_pool_new_factory_contract(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
) {
    if !is_resource_pool_infallible_constructor(callee) {
        return;
    }
    let Some(create) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("create"))
        .or_else(|| args.first())
    else {
        return;
    };
    let Some(fallible) = resource_pool_fallible_factory_expr(&create.value) else {
        return;
    };
    let type_name = hir_expr_type_name(fallible).unwrap_or("Result<T, E>");
    resource_pool_fallible_factory_diagnostic(analyzer, hir_expr_span(fallible).clone(), type_name);
}

fn check_resource_pool_try_new_factory_contract(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
) {
    if !is_resource_pool_fallible_constructor(callee) {
        return;
    }
    let Some(create) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("create"))
        .or_else(|| args.first())
    else {
        return;
    };
    if resource_pool_fallible_factory_expr(&create.value).is_none() {
        resource_pool_infallible_try_factory_diagnostic(
            analyzer,
            hir_expr_span(&create.value).clone(),
            hir_expr_type_name(&create.value).unwrap_or("T"),
        );
    }
}

fn check_resource_pool_constructor_max_size_contract(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
) {
    if !is_resource_pool_constructor(callee) {
        return;
    }
    // Lazy pools create on demand, so `max_size` is a runtime soft cap and may be
    // any `Int` (e.g. a config-derived value).
    if is_resource_pool_lazy(callee) || is_resource_pool_try_lazy(callee) {
        return;
    }
    let Some(max_size) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("max_size"))
        .or_else(|| args.get(1))
    else {
        return;
    };
    // Eager pools allocate up front, so `max_size` must be a statically known
    // positive value: a positive `Int` literal, or a reference to a positive
    // `Int` `const` (a reviewable, controlled constant).
    if resource_pool_static_positive_int(analyzer, &max_size.value).is_none() {
        resource_pool_invalid_max_size_diagnostic(analyzer, hir_expr_span(&max_size.value).clone());
    }
}

/// Resolve `expr` to a statically known positive `i64`: a positive `Int` literal,
/// or an `Ident` naming a `const` whose value is a positive `Int` literal.
fn resource_pool_static_positive_int(analyzer: &Analyzer<'_>, expr: &HirExpr) -> Option<i64> {
    match expr {
        HirExpr::Number { value, .. } => value.parse::<i64>().ok().filter(|value| *value > 0),
        HirExpr::Ident { name, .. } => analyzer.syntax_program.items.iter().find_map(|item| {
            let Item::Const(decl) = item else {
                return None;
            };
            if &decl.name != name {
                return None;
            }
            let crate::syntax::ast::Expr::Number(literal, _) = &decl.value else {
                return None;
            };
            literal.parse::<i64>().ok().filter(|value| *value > 0)
        }),
        _ => None,
    }
}

fn check_resource_pool_factory_resource_captures(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    state: &BodyState,
) {
    if !is_resource_pool_constructor(callee) {
        return;
    }
    let Some(create) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("create"))
        .or_else(|| args.first())
    else {
        return;
    };
    let mut captures = Vec::new();
    collect_resource_pool_factory_resource_captures(&create.value, state, &mut captures);
    for (name, span) in captures {
        resource_pool_factory_resource_capture_diagnostic(analyzer, &name, span);
    }
}

fn collect_resource_pool_factory_resource_captures(
    expr: &HirExpr,
    state: &BodyState,
    captures: &mut Vec<(String, Span)>,
) {
    if let HirExpr::Closure { params, body, .. } = expr {
        let mut bound = params.iter().cloned().collect::<HashSet<_>>();
        collect_resource_pool_factory_resource_captures_block(body, state, &mut bound, captures);
    }
}

fn collect_resource_pool_factory_resource_captures_block(
    block: &HirBlock,
    state: &BodyState,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    for statement in &block.statements {
        collect_resource_pool_factory_resource_captures_stmt(statement, state, bound, captures);
    }
}

fn collect_resource_pool_factory_resource_captures_stmt(
    statement: &HirStmt,
    state: &BodyState,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match statement {
        HirStmt::Let {
            name,
            value: Some(value),
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
            bound.insert(name.clone());
        }
        HirStmt::Let {
            name, value: None, ..
        } => {
            bound.insert(name.clone());
        }
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(resource, state, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut scoped,
                captures,
            );
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(condition, state, bound, captures);
            collect_resource_pool_factory_resource_captures_block(
                then_body,
                state,
                &mut bound.clone(),
                captures,
            );
            if let Some(else_body) = else_body {
                collect_resource_pool_factory_resource_captures_block(
                    else_body,
                    state,
                    &mut bound.clone(),
                    captures,
                );
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_resource_pool_factory_resource_captures_expr(
                    condition, state, bound, captures,
                );
            }
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut bound.clone(),
                captures,
            );
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_resource_pool_factory_resource_captures_expr(iterable, state, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut scoped,
                captures,
            );
        }
        HirStmt::Match { value, arms, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
            for arm in arms {
                let mut scoped = bound.clone();
                for binding in arm.pattern.binding_names() {
                    scoped.insert(binding.to_string());
                }
                collect_resource_pool_factory_resource_captures_block(
                    &arm.body,
                    state,
                    &mut scoped,
                    captures,
                );
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_resource_pool_factory_resource_captures_expr(
                    &arm.operation,
                    state,
                    bound,
                    captures,
                );
                let mut scoped = bound.clone();
                if arm.binding != "_" {
                    scoped.insert(arm.binding.clone());
                }
                collect_resource_pool_factory_resource_captures_block(
                    &arm.body,
                    state,
                    &mut scoped,
                    captures,
                );
            }
        }
        HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_resource_pool_factory_resource_captures_expr(
    expr: &HirExpr,
    state: &BodyState,
    bound: &HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if !bound.contains(name) && state.is_resource(name) {
                push_resource_pool_factory_capture(captures, name.clone(), span.clone());
            }
        }
        HirExpr::Field { base, .. } => {
            collect_resource_pool_factory_resource_captures_expr(base, state, bound, captures);
        }
        HirExpr::Index { base, index, .. } => {
            collect_resource_pool_factory_resource_captures_expr(base, state, bound, captures);
            collect_resource_pool_factory_resource_captures_expr(index, state, bound, captures);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_resource_pool_factory_resource_captures_expr(
                    &arg.value, state, bound, captures,
                );
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_resource_pool_factory_resource_captures_expr(left, state, bound, captures);
            collect_resource_pool_factory_resource_captures_expr(right, state, bound, captures);
        }
        HirExpr::Closure { params, body, .. } => {
            let mut scoped = bound.clone();
            scoped.extend(params.iter().cloned());
            collect_resource_pool_factory_resource_captures_block(
                body,
                state,
                &mut scoped,
                captures,
            );
        }
        HirExpr::Match { value, arms, .. } => {
            collect_resource_pool_factory_resource_captures_expr(value, state, bound, captures);
            for arm in arms {
                collect_resource_pool_factory_resource_captures_block(
                    &arm.body,
                    state,
                    &mut bound.clone(),
                    captures,
                );
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_resource_pool_factory_resource_captures_expr(
                    &entry.key, state, bound, captures,
                );
                collect_resource_pool_factory_resource_captures_expr(
                    &entry.value,
                    state,
                    bound,
                    captures,
                );
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_resource_pool_factory_capture(
    captures: &mut Vec<(String, Span)>,
    name: String,
    span: Span,
) {
    if !captures
        .iter()
        .any(|(existing_name, existing_span)| existing_name == &name && existing_span == &span)
    {
        captures.push((name, span));
    }
}

/// Reject lazy pool factories (`owned Fn`) that capture anything other than an
/// owned `local`: the factory is stored and called on demand, so it must own what
/// it captures. A parameter is borrowed; a managed binding is shared/refcounted —
/// neither is a clean owned value to move in. Names that are not bindings (globals,
/// consts, functions) are `'static` and allowed.
fn check_lazy_factory_captures_block(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    bindings: &std::collections::HashMap<String, HirBindingKind>,
) {
    for statement in &block.statements {
        check_lazy_factory_captures_stmt(analyzer, statement, bindings);
    }
}

fn check_lazy_factory_captures_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    bindings: &std::collections::HashMap<String, HirBindingKind>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_lazy_factory_captures_expr(analyzer, value, bindings);
            }
        }
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            check_lazy_factory_captures_expr(analyzer, value, bindings)
        }
        HirStmt::With { resource, body, .. } => {
            check_lazy_factory_captures_expr(analyzer, resource, bindings);
            check_lazy_factory_captures_block(analyzer, body, bindings);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_lazy_factory_captures_expr(analyzer, condition, bindings);
            check_lazy_factory_captures_block(analyzer, then_body, bindings);
            if let Some(else_body) = else_body {
                check_lazy_factory_captures_block(analyzer, else_body, bindings);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_lazy_factory_captures_expr(analyzer, condition, bindings);
            }
            check_lazy_factory_captures_block(analyzer, body, bindings);
        }
        HirStmt::For { iterable, body, .. } => {
            check_lazy_factory_captures_expr(analyzer, iterable, bindings);
            check_lazy_factory_captures_block(analyzer, body, bindings);
        }
        HirStmt::Match { value, arms, .. } => {
            check_lazy_factory_captures_expr(analyzer, value, bindings);
            for arm in arms {
                check_lazy_factory_captures_block(analyzer, &arm.body, bindings);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_lazy_factory_captures_expr(analyzer, &arm.operation, bindings);
                check_lazy_factory_captures_block(analyzer, &arm.body, bindings);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn check_lazy_factory_captures_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    bindings: &std::collections::HashMap<String, HirBindingKind>,
) {
    if let HirExpr::Call { callee, args, .. } = expr
        && (is_resource_pool_lazy(callee) || is_resource_pool_try_lazy(callee))
    {
        if let Some(create) = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("create"))
            .or_else(|| args.first())
            && let HirExpr::Closure { params, body, .. } = &create.value
        {
            let mut captures = Vec::new();
            collect_lazy_factory_capture_idents(params, body, &mut captures);
            for (name, span) in captures {
                match bindings.get(&name) {
                    // Owned local: the only kind that can be cleanly moved in.
                    Some(HirBindingKind::LocalLet) => {}
                    // Not a binding at all (global/const/function): `'static`, fine.
                    None => {}
                    Some(kind) => {
                        resource_pool_lazy_factory_capture_diagnostic(analyzer, &name, *kind, span);
                    }
                }
            }
        }
    }
    // Recurse so nested lazy constructors are also checked.
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_lazy_factory_captures_expr(analyzer, &arg.value, bindings);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => check_lazy_factory_captures_expr(analyzer, value, bindings),
        HirExpr::Binary { left, right, .. } => {
            check_lazy_factory_captures_expr(analyzer, left, bindings);
            check_lazy_factory_captures_expr(analyzer, right, bindings);
        }
        HirExpr::Field { base, .. } => check_lazy_factory_captures_expr(analyzer, base, bindings),
        HirExpr::Index { base, index, .. } => {
            check_lazy_factory_captures_expr(analyzer, base, bindings);
            check_lazy_factory_captures_expr(analyzer, index, bindings);
        }
        HirExpr::Closure { body, .. } => {
            check_lazy_factory_captures_block(analyzer, body, bindings)
        }
        HirExpr::Match { value, arms, .. } => {
            check_lazy_factory_captures_expr(analyzer, value, bindings);
            for arm in arms {
                check_lazy_factory_captures_block(analyzer, &arm.body, bindings);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_lazy_factory_captures_expr(analyzer, &entry.key, bindings);
                check_lazy_factory_captures_expr(analyzer, &entry.value, bindings);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_lazy_factory_capture_idents(
    params: &[String],
    block: &HirBlock,
    captures: &mut Vec<(String, Span)>,
) {
    let mut bound = params.iter().cloned().collect();
    collect_lazy_factory_capture_idents_block(block, &mut bound, captures);
}

fn collect_lazy_factory_capture_idents_block(
    block: &HirBlock,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    for statement in &block.statements {
        collect_lazy_factory_capture_idents_stmt(statement, bound, captures);
    }
}

fn collect_lazy_factory_capture_idents_stmt(
    statement: &HirStmt,
    bound: &mut HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match statement {
        HirStmt::Let {
            name,
            value: Some(value),
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
            bound.insert(name.clone());
        }
        HirStmt::Let {
            name, value: None, ..
        } => {
            bound.insert(name.clone());
        }
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures)
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(resource, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_lazy_factory_capture_idents_block(body, &mut scoped, captures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(condition, bound, captures);
            collect_lazy_factory_capture_idents_block(then_body, &mut bound.clone(), captures);
            if let Some(else_body) = else_body {
                collect_lazy_factory_capture_idents_block(else_body, &mut bound.clone(), captures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_lazy_factory_capture_idents_expr(condition, bound, captures);
            }
            collect_lazy_factory_capture_idents_block(body, &mut bound.clone(), captures);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_lazy_factory_capture_idents_expr(iterable, bound, captures);
            let mut scoped = bound.clone();
            scoped.insert(binding.clone());
            collect_lazy_factory_capture_idents_block(body, &mut scoped, captures);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
            for arm in arms {
                let mut scoped = bound.clone();
                for binding in arm.pattern.binding_names() {
                    scoped.insert(binding.to_string());
                }
                collect_lazy_factory_capture_idents_block(&arm.body, &mut scoped, captures);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_lazy_factory_capture_idents_expr(&arm.operation, bound, captures);
                let mut scoped = bound.clone();
                if arm.binding != "_" {
                    scoped.insert(arm.binding.clone());
                }
                collect_lazy_factory_capture_idents_block(&arm.body, &mut scoped, captures);
            }
        }
        HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_lazy_factory_capture_idents_expr(
    expr: &HirExpr,
    bound: &HashSet<String>,
    captures: &mut Vec<(String, Span)>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if !bound.contains(name) {
                push_resource_pool_factory_capture(captures, name.clone(), span.clone());
            }
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_lazy_factory_capture_idents_expr(&arg.value, bound, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_lazy_factory_capture_idents_expr(left, bound, captures);
            collect_lazy_factory_capture_idents_expr(right, bound, captures);
        }
        HirExpr::Field { base, .. } => {
            collect_lazy_factory_capture_idents_expr(base, bound, captures)
        }
        HirExpr::Index { base, index, .. } => {
            collect_lazy_factory_capture_idents_expr(base, bound, captures);
            collect_lazy_factory_capture_idents_expr(index, bound, captures);
        }
        HirExpr::Closure { params, body, .. } => {
            let mut scoped = bound.clone();
            scoped.extend(params.iter().cloned());
            collect_lazy_factory_capture_idents_block(body, &mut scoped, captures);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_lazy_factory_capture_idents_expr(value, bound, captures);
            for arm in arms {
                let mut scoped = bound.clone();
                for binding in arm.pattern.binding_names() {
                    scoped.insert(binding.to_string());
                }
                collect_lazy_factory_capture_idents_block(&arm.body, &mut scoped, captures);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_lazy_factory_capture_idents_expr(&entry.key, bound, captures);
                collect_lazy_factory_capture_idents_expr(&entry.value, bound, captures);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn is_resource_pool_discard(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "discard")
}

/// `discard`'s argument must be a bare local binding that is an active lease — the
/// root identifier of `mut <name>` (peeling the effect wrapper).
fn discard_lease_root(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Effect { value, .. } => discard_lease_root(value),
        HirExpr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

/// Validate that every `ResourcePool.discard` targets a binding introduced by an
/// enclosing `with ResourcePool.borrow/try_borrow(...) as name`. `lease_bindings`
/// is the stack of such names currently in scope.
fn check_resource_pool_discards(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    lease_bindings: &mut Vec<String>,
) {
    for statement in &block.statements {
        check_resource_pool_discards_stmt(analyzer, statement, lease_bindings);
    }
}

fn check_resource_pool_discards_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    lease_bindings: &mut Vec<String>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_resource_pool_discards_expr(analyzer, value, lease_bindings);
            }
        }
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings)
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            check_resource_pool_discards_expr(analyzer, resource, lease_bindings);
            // A `with` over a pool lease producer makes `binding` a lease.
            if resource_pool_borrow_pool_path(resource).is_some() {
                lease_bindings.push(binding.clone());
                check_resource_pool_discards(analyzer, body, lease_bindings);
                lease_bindings.pop();
            } else {
                check_resource_pool_discards(analyzer, body, lease_bindings);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_pool_discards_expr(analyzer, condition, lease_bindings);
            check_resource_pool_discards(analyzer, then_body, lease_bindings);
            if let Some(else_body) = else_body {
                check_resource_pool_discards(analyzer, else_body, lease_bindings);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_pool_discards_expr(analyzer, condition, lease_bindings);
            }
            check_resource_pool_discards(analyzer, body, lease_bindings);
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_pool_discards_expr(analyzer, iterable, lease_bindings);
            check_resource_pool_discards(analyzer, body, lease_bindings);
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings);
            for arm in arms {
                check_resource_pool_discards(analyzer, &arm.body, lease_bindings);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_pool_discards_expr(analyzer, &arm.operation, lease_bindings);
                check_resource_pool_discards(analyzer, &arm.body, lease_bindings);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn check_resource_pool_discards_expr(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    lease_bindings: &mut Vec<String>,
) {
    match expr {
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if is_resource_pool_discard(callee) {
                let targets_lease = args
                    .iter()
                    .find(|arg| arg.name.as_deref() == Some("lease"))
                    .or_else(|| args.first())
                    .and_then(|arg| discard_lease_root(&arg.value))
                    .is_some_and(|root| lease_bindings.iter().any(|name| name == root));
                if !targets_lease {
                    resource_pool_discard_not_lease_diagnostic(analyzer, span.clone());
                }
            }
            for arg in args {
                check_resource_pool_discards_expr(analyzer, &arg.value, lease_bindings);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings);
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_pool_discards_expr(analyzer, left, lease_bindings);
            check_resource_pool_discards_expr(analyzer, right, lease_bindings);
        }
        HirExpr::Field { base, .. } => {
            check_resource_pool_discards_expr(analyzer, base, lease_bindings)
        }
        HirExpr::Index { base, index, .. } => {
            check_resource_pool_discards_expr(analyzer, base, lease_bindings);
            check_resource_pool_discards_expr(analyzer, index, lease_bindings);
        }
        HirExpr::Closure { body, .. } => {
            // A closure escapes the lease scope, so it carries no lease bindings.
            check_resource_pool_discards(analyzer, body, &mut Vec::new());
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_pool_discards_expr(analyzer, value, lease_bindings);
            for arm in arms {
                check_resource_pool_discards(analyzer, &arm.body, lease_bindings);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_pool_discards_expr(analyzer, &entry.key, lease_bindings);
                check_resource_pool_discards_expr(analyzer, &entry.value, lease_bindings);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn resource_pool_borrow_pool_path(expr: &HirExpr) -> Option<PlacePath> {
    match expr {
        HirExpr::Call { callee, args, .. } if is_resource_pool_lease_producer(callee) => args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("pool"))
            .or_else(|| args.first())
            .and_then(|arg| effect_wrapped_place_path(&arg.value)),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            resource_pool_borrow_pool_path(value)
        }
        _ => None,
    }
}

fn effect_wrapped_place_path(expr: &HirExpr) -> Option<PlacePath> {
    match expr {
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            effect_wrapped_place_path(value)
        }
        _ => place_path(expr),
    }
}

fn resource_pool_fallible_factory_expr(expr: &HirExpr) -> Option<&HirExpr> {
    if hir_expr_type_name(expr).is_some_and(is_result_type) {
        return Some(expr);
    }
    match expr {
        HirExpr::Closure { body, .. } => resource_pool_fallible_factory_block(body),
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => resource_pool_fallible_factory_expr(value),
        HirExpr::Try { .. }
        | HirExpr::Match { .. }
        | HirExpr::Call { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Binary { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn resource_pool_fallible_factory_block(block: &HirBlock) -> Option<&HirExpr> {
    for statement in &block.statements {
        if let Some(expr) = resource_pool_fallible_factory_stmt(statement) {
            return Some(expr);
        }
    }
    None
}

fn resource_pool_fallible_factory_stmt(statement: &HirStmt) -> Option<&HirExpr> {
    match statement {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => resource_pool_fallible_factory_expr(value),
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => resource_pool_fallible_factory_block(then_body).or_else(|| {
            else_body
                .as_ref()
                .and_then(resource_pool_fallible_factory_block)
        }),
        HirStmt::Loop { body, .. } | HirStmt::With { body, .. } => {
            resource_pool_fallible_factory_block(body)
        }
        HirStmt::For { body, .. } => resource_pool_fallible_factory_block(body),
        HirStmt::Match { arms, .. } => arms
            .iter()
            .find_map(|arm| resource_pool_fallible_factory_block(&arm.body)),
        HirStmt::Select { arms, .. } => arms
            .iter()
            .find_map(|arm| resource_pool_fallible_factory_block(&arm.body)),
        HirStmt::Let { .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn check_resource_pool_factory_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => check_resource_producer_expr(analyzer, value, true),
        HirStmt::Let {
            value: Some(value), ..
        } => check_resource_producer_expr(analyzer, value, false),
        HirStmt::With { resource, body, .. } => {
            check_resource_producer_expr(analyzer, resource, true);
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_producer_expr(analyzer, condition, false);
            for statement in &then_body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    check_resource_pool_factory_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_producer_expr(analyzer, condition, false);
            }
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_producer_expr(analyzer, iterable, false);
            for statement in &body.statements {
                check_resource_pool_factory_stmt(analyzer, statement);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_producer_expr(analyzer, value, false);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_pool_factory_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_producer_expr(analyzer, &arm.operation, false);
                for statement in &arm.body.statements {
                    check_resource_pool_factory_stmt(analyzer, statement);
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

fn expr_is_resource_producer(analyzer: &Analyzer<'_>, expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { .. } => {
            expr_type_is_resource(analyzer, expr)
                || result_resource_ok_type(analyzer, expr).is_some()
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            expr_type_is_resource(analyzer, expr) && expr_is_resource_producer(analyzer, value)
        }
        _ => false,
    }
}

fn expr_type_is_resource(analyzer: &Analyzer<'_>, expr: &HirExpr) -> bool {
    hir_expr_type_name(expr).is_some_and(|type_name| {
        analyzer.hir.type_kind(type_root_name(type_name)) == Some(HirTypeKind::Resource)
    })
}

fn result_resource_ok_type(analyzer: &Analyzer<'_>, expr: &HirExpr) -> Option<String> {
    let type_name = hir_expr_type_name(expr)?;
    let ok_type = result_ok_type_name(type_name)?;
    if analyzer.hir.type_kind(type_root_name(ok_type)) == Some(HirTypeKind::Resource) {
        Some(ok_type.to_string())
    } else {
        None
    }
}

fn result_ok_type_name(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).into_iter().next()
}

fn result_error_type_name(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).get(1).copied()
}

fn list_element_type(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("List<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).into_iter().next()
}

fn stream_item_type(type_name: &str) -> Option<&str> {
    let inner = type_name
        .strip_prefix("Stream<")
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    split_top_level_type_args(inner).into_iter().next()
}

fn strip_fresh_type(type_name: &str) -> &str {
    type_name
        .strip_prefix("fresh ")
        .map(str::trim)
        .unwrap_or(type_name)
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if start < args.len() {
        parts.push(args[start..].trim());
    }
    parts
}

fn check_resource_pool_lease_stmt(analyzer: &mut Analyzer<'_>, statement: &HirStmt) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => check_resource_pool_lease_expr(analyzer, value, false),
        HirStmt::With { resource, body, .. } => {
            check_resource_pool_lease_expr(analyzer, resource, true);
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_pool_lease_expr(analyzer, condition, false);
            for statement in &then_body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
            if let Some(else_body) = else_body {
                for statement in &else_body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_pool_lease_expr(analyzer, condition, false);
            }
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_pool_lease_expr(analyzer, iterable, false);
            for statement in &body.statements {
                check_resource_pool_lease_stmt(analyzer, statement);
            }
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_pool_lease_expr(analyzer, value, false);
            for arm in arms {
                for statement in &arm.body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
                }
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_pool_lease_expr(analyzer, &arm.operation, false);
                for statement in &arm.body.statements {
                    check_resource_pool_lease_stmt(analyzer, statement);
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

fn check_resource_pool_active_lease_block(
    analyzer: &mut Analyzer<'_>,
    active_pool: &PlacePath,
    block: &HirBlock,
) {
    for statement in &block.statements {
        check_resource_pool_active_lease_stmt(analyzer, active_pool, statement);
    }
}

fn check_resource_pool_active_lease_stmt(
    analyzer: &mut Analyzer<'_>,
    active_pool: &PlacePath,
    statement: &HirStmt,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value)
        }
        HirStmt::With { resource, body, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, resource);
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, condition);
            check_resource_pool_active_lease_block(analyzer, active_pool, then_body);
            if let Some(else_body) = else_body {
                check_resource_pool_active_lease_block(analyzer, active_pool, else_body);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_resource_pool_active_lease_expr(analyzer, active_pool, condition);
            }
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirStmt::For { iterable, body, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, iterable);
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirStmt::Match { value, arms, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
            for arm in arms {
                check_resource_pool_active_lease_block(analyzer, active_pool, &arm.body);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_resource_pool_active_lease_expr(analyzer, active_pool, &arm.operation);
                check_resource_pool_active_lease_block(analyzer, active_pool, &arm.body);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn check_resource_pool_active_lease_expr(
    analyzer: &mut Analyzer<'_>,
    active_pool: &PlacePath,
    expr: &HirExpr,
) {
    match expr {
        HirExpr::Effect {
            effect,
            value,
            span,
            ..
        } => {
            if let Some(path) = place_path(value)
                && place_paths_overlap(active_pool, &path)
            {
                resource_pool_active_lease_conflict_diagnostic(analyzer, *effect, span.clone());
            }
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
        }
        HirExpr::Manage { value, span, .. } => {
            if let Some(path) = place_path(value)
                && place_paths_overlap(active_pool, &path)
            {
                resource_pool_active_lease_conflict_diagnostic(
                    analyzer,
                    ParamEffect::Take,
                    span.clone(),
                );
            }
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_resource_pool_active_lease_expr(analyzer, active_pool, &arg.value);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, left);
            check_resource_pool_active_lease_expr(analyzer, active_pool, right);
        }
        HirExpr::Field { base, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, base);
        }
        HirExpr::Index { base, index, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, base);
            check_resource_pool_active_lease_expr(analyzer, active_pool, index);
        }
        HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
        }
        HirExpr::Closure { body, .. } => {
            check_resource_pool_active_lease_block(analyzer, active_pool, body);
        }
        HirExpr::Match { value, arms, .. } => {
            check_resource_pool_active_lease_expr(analyzer, active_pool, value);
            for arm in arms {
                check_resource_pool_active_lease_block(analyzer, active_pool, &arm.body);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_resource_pool_active_lease_expr(analyzer, active_pool, &entry.key);
                check_resource_pool_active_lease_expr(analyzer, active_pool, &entry.value);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn place_paths_overlap(left: &PlacePath, right: &PlacePath) -> bool {
    if left.base != right.base {
        return false;
    }
    left.components
        .iter()
        .zip(&right.components)
        .all(|(left, right)| left == right)
}

fn is_resource_pool_borrow(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "borrow")
}

fn is_resource_pool_try_borrow(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_borrow")
}

/// Both `borrow` and `try_borrow` produce a scoped lease, so both are subject to
/// the same "must be consumed by `with`" and active-lease rules.
fn is_resource_pool_lease_producer(callee: &Callee) -> bool {
    is_resource_pool_borrow(callee) || is_resource_pool_try_borrow(callee)
}

fn is_resource_pool_new(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "new")
}

fn is_resource_pool_try_new(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_new")
}

fn is_resource_pool_lazy(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "lazy")
}

fn is_resource_pool_try_lazy(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_lazy")
}

/// Infallible factory contract (`new`/`lazy`): the `create` closure returns a
/// bare resource. `try_new`/`try_lazy` are the fallible counterparts.
fn is_resource_pool_infallible_constructor(callee: &Callee) -> bool {
    is_resource_pool_new(callee) || is_resource_pool_lazy(callee)
}

fn is_resource_pool_fallible_constructor(callee: &Callee) -> bool {
    is_resource_pool_try_new(callee) || is_resource_pool_try_lazy(callee)
}

fn is_resource_pool_constructor(callee: &Callee) -> bool {
    is_resource_pool_infallible_constructor(callee) || is_resource_pool_fallible_constructor(callee)
}

fn type_root_name(type_name: &str) -> &str {
    let type_name = type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name.trim());
    type_name
        .split_once('<')
        .map_or(type_name, |(root, _)| root)
}

fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let start = type_name.find('<')?;
    let end = type_name.rfind('>')?;
    if end <= start {
        return None;
    }
    let inner = &type_name[start + 1..end];
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut part_start = 0usize;
    for (index, ch) in inner.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(inner[part_start..index].trim());
                part_start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = inner[part_start..].trim();
    if !tail.is_empty() {
        args.push(tail);
    }
    Some(args)
}

fn is_resource_pool_type(type_name: &str) -> bool {
    type_root_name(type_name) == "ResourcePool"
}

fn resource_is_active_at(
    local_analysis: &LocalAnalysis,
    binding: &str,
    span: &crate::diagnostic::Span,
) -> bool {
    local_analysis
        .flow_entry_state(span)
        .is_none_or(|state| state.is_resource(binding))
}

fn resource_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource `{binding}` cannot escape its `with` block."),
            span,
            "resource escapes",
        )
        .with_cause("A `with` resource must be dropped when the block exits."),
    );
}

fn resource_producer_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    type_name: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource-producing expression of type `{type_name}` must be consumed by `with`."),
            span,
            "resource producer escapes",
        )
        .with_cause("Resource-producing calls create transient linear values that cannot be stored, returned, retained, managed, or passed as ordinary values.")
        .with_fix(
            "use_with",
            "Use `with producer(...)? as resource { ... }`, or an approved resource container API.",
            "manual",
        ),
    );
}

fn resource_pool_lease_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            "resource lease from `ResourcePool.borrow` must be scoped by `with`.",
            span,
            "resource lease escapes",
        )
        .with_cause("Pool leases are resources and must be returned to the pool when the `with` block exits.")
        .with_fix(
            "wrap_with",
            "Use `with ResourcePool.borrow(pool: mut pool) as lease { ... }`.",
            "manual",
        ),
    );
}

fn resource_pool_not_local_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_NOT_LOCAL,
            format!("ResourcePool binding `{binding}` must be local."),
            span,
            "ResourcePool must be local",
        )
        .with_cause("ResourcePool owns long-lived resources and must not be hidden behind an ordinary managed binding.")
        .with_fix(
            "make_resource_pool_local",
            format!("Declare `{binding}` with `local`, or pass it as a `mut` local-capability parameter."),
            "machine-applicable",
        ),
    );
}

fn resource_pool_fallible_factory_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    type_name: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_FALLIBLE_FACTORY,
            "`ResourcePool.new` requires an infallible factory.",
            span,
            "fallible ResourcePool factory",
        )
        .with_cause(format!(
            "The factory expression has type `{type_name}`, but `ResourcePool.new` expects a resource value."
        ))
        .with_cause("v0.6 `ResourcePool.new` eagerly constructs the pool and does not hide `Result` handling inside the pool constructor.")
        .with_fix(
            "use_matching_pool_constructor",
            "Handle failure before constructing the pool, or use `ResourcePool.try_new` with a fallible factory.",
            "manual",
        ),
    );
}

fn resource_pool_infallible_try_factory_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
    type_name: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_FALLIBLE_FACTORY,
            "`ResourcePool.try_new` requires a fallible factory.",
            span,
            "infallible ResourcePool try factory",
        )
        .with_cause(format!(
            "The factory expression has type `{type_name}`, but `ResourcePool.try_new` expects `Result<T, E>`."
        ))
        .with_cause("Use `ResourcePool.new` for infallible eager construction, or return a `Result` from the factory closure.")
        .with_fix(
            "use_matching_pool_constructor",
            "Use `ResourcePool.new`, or call a fallible resource constructor from the factory.",
            "manual",
        ),
    );
}

fn resource_pool_lazy_factory_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    kind: HirBindingKind,
    span: crate::diagnostic::Span,
) {
    let (noun, why) = match kind {
        HirBindingKind::Param => (
            "parameter",
            "a parameter is borrowed and would not outlive the pool",
        ),
        HirBindingKind::ManagedLet => (
            "managed binding",
            "a managed binding is shared/refcounted, not a clean owned value",
        ),
        HirBindingKind::LocalLet => ("binding", "it is not a clean owned local"),
    };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_LAZY_FACTORY_CAPTURE,
            format!("lazy pool factory captures {noun} `{name}`."),
            span,
            "non-owned binding captured by stored factory",
        )
        .with_cause(format!(
            "A lazy (`owned Fn`) factory is stored in the pool and called on demand, so it must own its captures, but {why}."
        ))
        .with_fix(
            "capture_owned_local",
            format!("Bind an owned `local` from `{name}` before constructing the pool and capture that local instead."),
            "manual",
        ),
    );
}

fn resource_pool_discard_not_lease_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_DISCARD_NOT_LEASE,
            "`ResourcePool.discard` requires a pool lease binding.",
            span,
            "not a pool lease",
        )
        .with_cause(
            "`discard` evicts a checked-out lease, so its argument must be the binding of an enclosing `with ResourcePool.borrow(...) as name` or `with ResourcePool.try_borrow(...)? as name`.",
        )
        .with_fix(
            "discard_lease_binding",
            "Call `discard` on the `with`-lease binding, e.g. `ResourcePool.discard(lease: mut conn)` inside `with ... as conn`.",
            "manual",
        ),
    );
}

fn resource_pool_active_lease_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    effect: ParamEffect,
    span: crate::diagnostic::Span,
) {
    let effect = match effect {
        ParamEffect::Read => "read",
        ParamEffect::Mut => "mut",
        ParamEffect::Take => "take/manage",
    };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_ACTIVE_LEASE_CONFLICT,
            "ResourcePool cannot be used while one of its leases is active.",
            span,
            "ResourcePool active lease conflict",
        )
        .with_cause(format!(
            "This `{effect}` use targets the same pool root as an active `ResourcePool.borrow` lease."
        ))
        .with_cause("As a conservative policy the language permits one active lease per pool: borrow, stats, reset, take, and manage of the same pool root are rejected until the `with` scope exits. (The runtime checkout model can track several leases at once; surfacing concurrent leases in the language is a deliberate future step.)")
        .with_fix(
            "end_pool_lease_scope",
            "Move this pool use after the `with ResourcePool.borrow(...)` block, or use a different pool.",
            "manual",
        ),
    );
}

fn resource_pool_invalid_max_size_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_POOL_INVALID_MAX_SIZE,
            "`ResourcePool.new` requires a positive literal `max_size`.",
            span,
            "invalid ResourcePool max_size",
        )
        .with_cause("v0.6 `ResourcePool.new` eagerly constructs exactly `max_size` resources and returns no `Result`.")
        .with_cause("Use a positive `Int` literal so empty or dynamically sized pools cannot hide construction failure or exhaustion behavior.")
        .with_fix(
            "use_positive_literal_max_size",
            "Pass a positive integer literal such as `max_size: 1`.",
            "manual",
        ),
    );
}

fn resource_pool_factory_resource_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("ResourcePool factory cannot capture resource `{binding}`."),
            span,
            "resource captured by ResourcePool factory",
        )
        .with_cause("ResourcePool factories are eager and noescape in v0.6, but they still must not close over with-bound resources.")
        .with_fix(
            "avoid_resource_capture",
            "Create the pooled resource directly inside the factory, or pass ordinary managed configuration into the factory.",
            "manual",
        ),
    );
}

fn local_class_binding_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::LOCAL_CLASS_BINDING,
            format!("class binding `{binding}` cannot be local."),
            span,
            "class bound as local",
        )
        .with_cause(
            "Classes are managed identity objects; their constructors produce managed handles.",
        )
        .with_fix(
            "use_managed_class_binding",
            format!("Declare `{binding}` with `let` instead of `local`."),
            "machine-applicable",
        ),
    );
}

fn invalid_manage_operand_diagnostic(
    analyzer: &mut Analyzer<'_>,
    cause: impl Into<String>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::INVALID_MANAGE_OPERAND,
            "`manage` requires a local binding.",
            span,
            "not a local binding",
        )
        .with_cause(cause)
        .with_fix(
            "remove_manage_or_create_local",
            "Remove `manage`, or create the value as `local` at its origin.",
            "manual",
        ),
    );
}

fn invalid_take_operand_diagnostic(
    analyzer: &mut Analyzer<'_>,
    cause: impl Into<String>,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::INVALID_TAKE_OPERAND,
            "`take` requires a local value.",
            span,
            "not a local value",
        )
        .with_cause(cause)
        .with_fix(
            "use_local_or_read",
            "Pass a local value with `take`, or use `read`/`mut` for managed values.",
            "manual",
        ),
    );
}

fn resource_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource `{binding}` cannot be captured by a managed closure."),
            span,
            "resource captured",
        )
        .with_cause("Managed closures may outlive the `with` block that owns the resource."),
    );
}

fn fresh_return_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function_name: &str,
    name: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::FRESH_RETURN_NOT_CLEAN,
            format!(
                "fresh function `{}` returns non-fresh value `{name}`.",
                function_name
            ),
            span,
            "non-fresh value returned",
        )
        .with_cause("A `fresh` return must be newly created or a clean local binding created inside the function.")
        .with_fix(
            "return_fresh_value",
            "Return a struct constructor, fresh call, or clean local binding created inside the function.",
            "manual",
        ),
    );
}

fn freshness_unknown_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function_name: &str,
    issue: FreshReturnIssue,
) {
    analyzer.diagnostics.push(
        Diagnostic::warning(
            code::FRESHNESS_UNKNOWN,
            format!("freshness of return value in `{function_name}` could not be proven."),
            issue.span,
            "freshness unknown",
        )
        .with_cause(
            "This MVP checker trusts clean locals, clean inline fields of locals, struct constructors, and known fresh functions.",
        ),
    );
}

fn invalid_fresh_return_type_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    target: &TypeRef,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::INVALID_FRESH_RETURN_TYPE,
            format!(
                "function `{}` declares `fresh {}` but `{}` is not a struct.",
                function.name, target.name, target.name
            ),
            target.span.clone(),
            "invalid fresh type",
        )
        .with_cause("RSScript `fresh` is a shallow guarantee for newly created struct shells.")
        .with_fix(
            "use_struct_fresh_type",
            "Return a struct type as fresh, or remove `fresh` from this return contract.",
            "manual",
        ),
    );
}

fn trusted_fresh_ident(analyzer: &Analyzer<'_>, name: &str) -> bool {
    analyzer.hir.type_kind(name) == Some(HirTypeKind::Struct)
        || analyzer
            .hir
            .resolve_function(None, name)
            .is_some_and(|signature| signature.returns_fresh)
}
