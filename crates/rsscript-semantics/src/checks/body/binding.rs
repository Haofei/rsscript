use super::*;

pub(super) fn check_block(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    block: &HirBlock,
    state: &mut BodyState,
    check_resource_contexts: bool,
    continuation_uses: &HashSet<String>,
) -> Flow {
    let Some(_recursion) = analyzer.budget.enter_recursion() else {
        return Flow::Fallthrough;
    };
    if !analyzer.budget.consume_nodes(block.statements.len().max(1)) {
        return Flow::Fallthrough;
    }
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

pub(super) fn block_live_after_statements(
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

pub(super) fn collect_stmt_uses(statement: &HirStmt, uses: &mut HashSet<String>) {
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
        HirStmt::Expr(expr) => collect_expr_uses(expr, uses),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                collect_expr_uses(read, uses);
            }
            collect_expr_uses(value, uses);
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

/// Report `let` bindings whose type cannot be inferred (RS0034).
///
/// A bare `Ok(..)`/`Err(..)`/`None` initializer leaves a type parameter open
/// (`Result`'s error type, `Result`'s ok type, or `Option`'s value type).
/// RSScript resolves that parameter from how the binding is later *used*; if the
/// binding is never used, nothing constrains it, the type is genuinely ambiguous,
/// and the program would not lower to valid Rust (rustc `E0282`). The "never
/// used" gate is what keeps this sound — a binding used downstream may have its
/// parameter pinned there (e.g. `let r = Ok(x); ... match Result.map(r) { Err(e)
/// => use e as String }`), so only the provably-unconstrained (unused) case is
/// rejected, which never false-positives on constrained-by-use code.
pub(super) fn check_uninferable_unused_bindings(
    analyzer: &mut Analyzer<'_>,
    body: &crate::hir::HirFunctionBody,
) {
    let Some(block) = body.block.as_ref() else {
        return;
    };
    // Every name referenced anywhere in the function, with binding structure
    // ignored (unlike `collect_block_uses`, which strips locally-bound names to
    // compute free variables). We need raw references so a binding used only as a
    // local — `return v`, `f(read v)` — counts as used.
    let mut uses = HashSet::new();
    collect_all_referenced_names_block(block, &mut uses);
    check_uninferable_bindings_in_block(analyzer, block, &uses);
}

/// A generic record construction must leave the front end with its type
/// arguments proved.
///
/// `lowerer.rs::lower_record_constructor` substitutes the call site's inferred
/// type arguments into the constructor's declared result, so `(1, "a")` is
/// recorded as `__Tuple2<Int, String>` and not as the declaration's own
/// parameter names (§2.9). The checker proves a call site's arguments all at
/// once or not at all; where it cannot, the constructed value has no type the
/// typed executable facts can name, and lowering refuses. That refusal is a
/// real rule, but it belongs to `rss check`, not to `rss build`: report it here,
/// at the construction, so a program never checks clean and then fails to
/// build.
pub(super) fn check_provable_generic_construction(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    resolution: &CallResolution,
    type_arguments: &[crate::ResolvedType],
    span: &Span,
) {
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
    if signature.type_params.is_empty() || type_arguments.len() == signature.type_params.len() {
        return;
    }
    let name = type_root_name(&body_callee_display(callee)).to_owned();
    // Tuples are surface sugar over `__TupleN`; naming the synthetic struct in
    // a diagnostic would point the reader at a declaration they never wrote.
    let subject = if name.starts_with("__Tuple") {
        "this tuple".to_owned()
    } else {
        format!("`{name}`")
    };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::UNINFERABLE_BINDING_TYPE,
            format!("the type arguments of {subject} cannot be inferred."),
            span.clone(),
            "uninferable construction type",
        )
        .with_cause(
            "A generic construction takes its type arguments from its arguments' types, and at least one argument here has no known type: an unannotated closure, a bare `None`, an empty `[]`, or a value whose own binding was never given a type.",
        )
        .with_fix(
            "annotate_construction_source",
            "Annotate the binding or parameter the untyped argument comes from (e.g. `let n: Int = ...`), so every type argument is proved at the construction.",
            "manual",
        ),
    );
}

pub(super) fn collect_all_referenced_names_block(block: &HirBlock, uses: &mut HashSet<String>) {
    for statement in &block.statements {
        collect_all_referenced_names_stmt(statement, uses);
    }
}

pub(super) fn collect_all_referenced_names_stmt(statement: &HirStmt, uses: &mut HashSet<String>) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_expr_uses(value, uses);
            }
        }
        HirStmt::With { resource, body, .. } => {
            collect_expr_uses(resource, uses);
            collect_all_referenced_names_block(body, uses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_uses(condition, uses);
            collect_all_referenced_names_block(then_body, uses);
            if let Some(else_body) = else_body {
                collect_all_referenced_names_block(else_body, uses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_uses(condition, uses);
            }
            collect_all_referenced_names_block(body, uses);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_uses(iterable, uses);
            collect_all_referenced_names_block(body, uses);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_uses(value, uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_uses(guard, uses);
                }
                collect_all_referenced_names_block(&arm.body, uses);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_uses(&arm.operation, uses);
                collect_all_referenced_names_block(&arm.body, uses);
            }
        }
        HirStmt::Expr(expr) => collect_expr_uses(expr, uses),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                collect_expr_uses(read, uses);
            }
            collect_expr_uses(value, uses);
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_uninferable_bindings_in_block(
    analyzer: &mut Analyzer<'_>,
    block: &HirBlock,
    uses: &HashSet<String>,
) {
    for statement in &block.statements {
        check_uninferable_bindings_in_stmt(analyzer, statement, uses);
    }
}

pub(super) fn check_uninferable_bindings_in_stmt(
    analyzer: &mut Analyzer<'_>,
    statement: &HirStmt,
    uses: &HashSet<String>,
) {
    match statement {
        // A bare `Ok`/`Err`/`None`/`[]` with no user annotation (the HIR sets
        // `ty == value_ty` exactly when the user did not annotate — an annotation
        // overrides the binding type with a placeholder-free structural type),
        // bound to a name that is never used, so nothing pins the open parameter.
        HirStmt::Let {
            name,
            value: Some(value),
            ty,
            value_ty,
            span,
            ..
        } if ty == value_ty && !uses.contains(name) && open_generic_initializer(value) => {
            analyzer
                .diagnostics
                .push(rsscript_semantics::uninferable_binding_type_diagnostic(
                    name,
                    span.clone(),
                ));
        }
        HirStmt::With { body, .. } | HirStmt::Loop { body, .. } | HirStmt::For { body, .. } => {
            check_uninferable_bindings_in_block(analyzer, body, uses);
        }
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            check_uninferable_bindings_in_block(analyzer, then_body, uses);
            if let Some(else_body) = else_body {
                check_uninferable_bindings_in_block(analyzer, else_body, uses);
            }
        }
        HirStmt::Match { arms, .. } => {
            for arm in arms {
                check_uninferable_bindings_in_block(analyzer, &arm.body, uses);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                check_uninferable_bindings_in_block(analyzer, &arm.body, uses);
            }
        }
        _ => {}
    }
}

/// Whether `value` leaves a type parameter unconstrained: a bare
/// `Ok(..)`/`Err(..)`/`None` constructor, or an empty list literal, whose
/// element type inference records as the `?` placeholder. `Some(x)` and a
/// non-empty `[x, …]` are fully determined by their contents, so both are
/// excluded.
pub(super) fn open_generic_initializer(value: &HirExpr) -> bool {
    match value {
        HirExpr::Call {
            callee: Callee::Name(name),
            ..
        } => matches!(name.as_str(), "Ok" | "Err" | "None"),
        HirExpr::Ident { name, .. } => name == "None",
        HirExpr::ArrayLiteral { items, .. } => items.is_empty(),
        _ => false,
    }
}

pub(super) fn collect_block_uses(block: &HirBlock, uses: &mut HashSet<String>) {
    let mut block_uses = HashSet::new();
    for statement in block.statements.iter().rev() {
        collect_stmt_uses(statement, &mut block_uses);
        remove_stmt_bindings(statement, &mut block_uses);
    }
    for name in block_uses {
        uses.insert(name);
    }
}

pub(super) fn collect_expr_uses(expr: &HirExpr, uses: &mut HashSet<String>) {
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
                if let Some(guard) = &arm.guard {
                    collect_expr_uses(guard, uses);
                }
                collect_block_uses(&arm.body, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_uses(&entry.key, uses);
                collect_expr_uses(&entry.value, uses);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_uses(&field.value, uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_uses(item, uses);
            }
        }
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn collect_await_operand_live_uses(expr: &HirExpr, uses: &mut HashSet<String>) {
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
                if let Some(guard) = &arm.guard {
                    collect_await_operand_live_uses(guard, uses);
                }
                collect_block_uses(&arm.body, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_await_operand_live_uses(&entry.key, uses);
                collect_await_operand_live_uses(&entry.value, uses);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_await_operand_live_uses(&field.value, uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_await_operand_live_uses(item, uses);
            }
        }
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn is_builtin_value_ident(name: &str) -> bool {
    matches!(name, "Unit" | "true" | "false" | "null")
}

pub(super) fn remove_stmt_bindings(statement: &HirStmt, uses: &mut HashSet<String>) {
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

#[cfg(test)]
mod tests {
    use crate::analyze_source;
    use crate::diagnostic::code;

    fn uninferable_count(source: &str) -> usize {
        analyze_source("uninferable-binding.rss", source)
            .iter()
            .filter(|diagnostic| diagnostic.code == code::UNINFERABLE_BINDING_TYPE)
            .count()
    }

    #[test]
    fn an_unused_empty_list_literal_is_as_uninferable_as_a_bare_ok() {
        assert_eq!(
            uninferable_count("fn main() -> Unit {\n    let xs = []\n    return Unit\n}\n"),
            1
        );
        assert_eq!(
            uninferable_count("fn main() -> Unit {\n    let value = Ok(1)\n    return Unit\n}\n"),
            1
        );
    }

    #[test]
    fn an_annotated_or_non_empty_list_literal_is_inferable() {
        assert_eq!(
            uninferable_count(
                "fn main() -> Unit {\n    let xs: List<Int> = []\n    return Unit\n}\n"
            ),
            0
        );
        assert_eq!(
            uninferable_count("fn main() -> Unit {\n    let xs = [1, 2]\n    return Unit\n}\n"),
            0
        );
    }
}
