use super::super::*;
pub(super) fn closure_capture_names(
    body: &HirBlock,
    params: &[String],
    explicit_captures: &[rsscript_exec_ir::ExecutableClosureCapture],
    outer_locals: &HashMap<String, Reg>,
) -> Vec<String> {
    let mut names = explicit_captures
        .iter()
        .map(|capture| capture.name.clone())
        .collect::<Vec<_>>();
    let mut seen = names.iter().cloned().collect::<HashSet<_>>();
    let mut bound = params.iter().cloned().collect::<HashSet<_>>();
    let mut free = BTreeSet::new();
    collect_free_locals_block(body, &mut bound, &mut free);
    for name in free {
        if outer_locals.contains_key(&name) && seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

pub(super) fn collect_free_locals_block(
    block: &HirBlock,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    for statement in &block.statements {
        collect_free_locals_stmt(statement, bound, free);
    }
}

pub(super) fn collect_free_locals_stmt(
    statement: &HirStmt,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match statement {
        HirStmt::Let { name, value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
            bound.insert(name.clone());
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_free_locals_expr(resource, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_free_locals_expr(condition, bound, free);
            collect_free_locals_block(&then_body.clone(), &mut bound.clone(), free);
            if let Some(else_body) = else_body {
                collect_free_locals_block(&else_body.clone(), &mut bound.clone(), free);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_free_locals_expr(condition, bound, free);
            }
            collect_free_locals_block(&body.clone(), &mut bound.clone(), free);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_free_locals_expr(iterable, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_free_locals_expr(&arm.operation, bound, free);
                let mut arm_bound = bound.clone();
                arm_bound.insert(arm.binding.clone());
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            collect_free_locals_expr(target, bound, free);
            collect_free_locals_expr(value, bound, free);
        }
        HirStmt::Expr(value) => collect_free_locals_expr(value, bound, free),
        HirStmt::Break | HirStmt::Continue | HirStmt::Unknown => {}
    }
}

pub(super) fn collect_free_locals_expr(
    expr: &HirExpr,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match expr {
        HirExpr::Ident { name, .. } => {
            if !bound.contains(name) {
                free.insert(name.clone());
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_free_locals_expr(&field.value, bound, free);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_free_locals_expr(&entry.key, bound, free);
                collect_free_locals_expr(&entry.value, bound, free);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_free_locals_expr(item, bound, free);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_free_locals_expr(left, bound, free);
            collect_free_locals_expr(right, bound, free);
        }
        HirExpr::Field { base, .. } => collect_free_locals_expr(base, bound, free),
        HirExpr::Index { base, index, .. } => {
            collect_free_locals_expr(base, bound, free);
            collect_free_locals_expr(index, bound, free);
        }
        HirExpr::Call { receiver, args, .. } => {
            if let Some(receiver) = receiver {
                collect_free_locals_expr(&receiver.value, bound, free);
            }
            for arg in args {
                collect_free_locals_expr(&arg.value, bound, free);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_free_locals_expr(value, bound, free),
        HirExpr::Closure {
            params,
            captures,
            body,
            ..
        } => {
            for capture in captures {
                if !bound.contains(&capture.name) {
                    free.insert(capture.name.clone());
                }
            }
            let mut nested_bound = bound.clone();
            for param in params {
                nested_bound.insert(param.clone());
            }
            collect_free_locals_block(body, &mut nested_bound, free);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown => {}
    }
}
