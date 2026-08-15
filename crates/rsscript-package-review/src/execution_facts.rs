use std::collections::BTreeSet;

use rsscript_syntax::ast::{Block, Callee, DataEffect, Expr, Item, Stmt};

use crate::PackageSource;
use rsscript_package_model::{
    PackageAnalysisResourceLifetime, PackageAnalysisResourceTransfer, PackageAnalysisTaskGroup,
    PackageReviewFileKind,
};

pub fn collect_execution_facts(
    sources: &[PackageSource],
    database: &rsscript_semantics::SemanticDatabase,
) -> (
    Vec<PackageAnalysisResourceLifetime>,
    Vec<PackageAnalysisResourceTransfer>,
    Vec<PackageAnalysisTaskGroup>,
) {
    let mut resources = Vec::new();
    let mut transfers = Vec::new();
    let mut task_groups = Vec::new();
    for (snapshot, program) in database
        .sources()
        .files()
        .iter()
        .zip(database.source_programs())
    {
        let Some(source) = sources.iter().find(|source| {
            source.kind == PackageReviewFileKind::Source && source.path == snapshot.path()
        }) else {
            continue;
        };
        let _relative_path = &source.relative_path;
        for item in &program.items {
            if let Item::Function(function) = item {
                collect_block(
                    &function.name,
                    &function.body,
                    &BTreeSet::new(),
                    &mut resources,
                    &mut transfers,
                    &mut task_groups,
                );
            }
        }
    }
    resources.sort();
    resources.dedup();
    transfers.sort();
    transfers.dedup();
    task_groups.sort();
    task_groups.dedup();
    (resources, transfers, task_groups)
}

fn collect_block(
    function: &str,
    block: &Block,
    managed_resources: &BTreeSet<String>,
    resources: &mut Vec<PackageAnalysisResourceLifetime>,
    transfers: &mut Vec<PackageAnalysisResourceTransfer>,
    task_groups: &mut Vec<PackageAnalysisTaskGroup>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::With(with) => {
                collect_transfers_in_expr(function, &with.resource, managed_resources, transfers);
                resources.push(PackageAnalysisResourceLifetime {
                    function: function.to_string(),
                    binding: with.binding.clone(),
                    acquisition: "with".to_string(),
                    cleanup: "scope_exit".to_string(),
                    cleanup_on_cancellation: true,
                });
                let mut nested_resources = managed_resources.clone();
                nested_resources.insert(with.binding.clone());
                collect_block(
                    function,
                    &with.body,
                    &nested_resources,
                    resources,
                    transfers,
                    task_groups,
                );
            }
            Stmt::TaskGroup(group) => {
                task_groups.push(PackageAnalysisTaskGroup {
                    function: function.to_string(),
                    spawned_tasks: count_async_bindings(&group.body),
                    select_arms: count_select_arms(&group.body),
                    drains_on_exit: true,
                    cleanup_on_cancellation: true,
                });
                collect_block(
                    function,
                    &group.body,
                    managed_resources,
                    resources,
                    transfers,
                    task_groups,
                );
            }
            Stmt::If(statement) => {
                collect_transfers_in_expr(
                    function,
                    &statement.condition,
                    managed_resources,
                    transfers,
                );
                collect_block(
                    function,
                    &statement.then_body,
                    managed_resources,
                    resources,
                    transfers,
                    task_groups,
                );
                if let Some(else_body) = &statement.else_body {
                    collect_block(
                        function,
                        else_body,
                        managed_resources,
                        resources,
                        transfers,
                        task_groups,
                    );
                }
            }
            Stmt::Loop(statement) => {
                if let Some(condition) = &statement.condition {
                    collect_transfers_in_expr(function, condition, managed_resources, transfers);
                }
                collect_block(
                    function,
                    &statement.body,
                    managed_resources,
                    resources,
                    transfers,
                    task_groups,
                );
            }
            Stmt::For(statement) => {
                collect_transfers_in_expr(
                    function,
                    &statement.iterable,
                    managed_resources,
                    transfers,
                );
                collect_block(
                    function,
                    &statement.body,
                    managed_resources,
                    resources,
                    transfers,
                    task_groups,
                );
            }
            Stmt::Match(statement) => {
                collect_transfers_in_expr(function, &statement.value, managed_resources, transfers);
                for arm in &statement.arms {
                    if let Some(guard) = &arm.guard {
                        collect_transfers_in_expr(function, guard, managed_resources, transfers);
                    }
                    collect_block(
                        function,
                        &arm.body,
                        managed_resources,
                        resources,
                        transfers,
                        task_groups,
                    );
                }
            }
            Stmt::Select(statement) => {
                for arm in &statement.arms {
                    collect_transfers_in_expr(
                        function,
                        &arm.operation,
                        managed_resources,
                        transfers,
                    );
                    collect_block(
                        function,
                        &arm.body,
                        managed_resources,
                        resources,
                        transfers,
                        task_groups,
                    );
                }
            }
            Stmt::LetElse(statement) => {
                collect_transfers_in_expr(function, &statement.value, managed_resources, transfers);
                collect_block(
                    function,
                    &statement.else_body,
                    managed_resources,
                    resources,
                    transfers,
                    task_groups,
                );
            }
            Stmt::Let(statement) => {
                if let Some(value) = &statement.value {
                    collect_transfers_in_expr(function, value, managed_resources, transfers);
                }
            }
            Stmt::Return(statement) => {
                if let Some(value) = &statement.value {
                    collect_transfers_in_expr(function, value, managed_resources, transfers);
                }
            }
            Stmt::Assign(statement) => {
                collect_transfers_in_expr(
                    function,
                    &statement.target,
                    managed_resources,
                    transfers,
                );
                collect_transfers_in_expr(function, &statement.value, managed_resources, transfers);
            }
            Stmt::Expr(expression) => {
                collect_transfers_in_expr(function, expression, managed_resources, transfers);
            }
            Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

fn collect_transfers_in_expr(
    function: &str,
    expression: &Expr,
    managed_resources: &BTreeSet<String>,
    transfers: &mut Vec<PackageAnalysisResourceTransfer>,
) {
    match expression {
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_transfers_in_expr(function, &field.value, managed_resources, transfers);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_transfers_in_expr(function, &entry.key, managed_resources, transfers);
                collect_transfers_in_expr(function, &entry.value, managed_resources, transfers);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_transfers_in_expr(function, item, managed_resources, transfers);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_transfers_in_expr(function, left, managed_resources, transfers);
            collect_transfers_in_expr(function, right, managed_resources, transfers);
        }
        Expr::Field { base, .. }
        | Expr::Manage { value: base, .. }
        | Expr::Spawn { value: base, .. }
        | Expr::Await { value: base, .. }
        | Expr::Try { value: base, .. } => {
            collect_transfers_in_expr(function, base, managed_resources, transfers);
        }
        Expr::Index { base, index, .. } => {
            collect_transfers_in_expr(function, base, managed_resources, transfers);
            collect_transfers_in_expr(function, index, managed_resources, transfers);
        }
        Expr::Call { callee, args, .. } => {
            if let Callee::ReceiverCall { receiver, .. } = callee {
                collect_transfers_in_expr(function, receiver, managed_resources, transfers);
            }
            for argument in args {
                collect_transfers_in_expr(function, &argument.value, managed_resources, transfers);
            }
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            ..
        } => {
            if let Expr::Ident(binding, _) = value.as_ref()
                && managed_resources.contains(binding)
            {
                transfers.push(PackageAnalysisResourceTransfer {
                    function: function.to_string(),
                    binding: binding.clone(),
                    operation: "take".to_string(),
                });
            }
            collect_transfers_in_expr(function, value, managed_resources, transfers);
        }
        Expr::Effect { value, .. } => {
            collect_transfers_in_expr(function, value, managed_resources, transfers);
        }
        Expr::Closure { body, .. } => {
            collect_block_for_transfers(function, body, managed_resources, transfers)
        }
        Expr::Match { value, arms, .. } => {
            collect_transfers_in_expr(function, value, managed_resources, transfers);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_transfers_in_expr(function, guard, managed_resources, transfers);
                }
                collect_block_for_transfers(function, &arm.body, managed_resources, transfers);
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

/// Expression-form closures/matches can contain the same explicit `take`
/// syntax as a statement block. Reuse the lexical walk without emitting a
/// second set of lifetime or task facts for an expression nested in a function.
fn collect_block_for_transfers(
    function: &str,
    block: &Block,
    managed_resources: &BTreeSet<String>,
    transfers: &mut Vec<PackageAnalysisResourceTransfer>,
) {
    let mut ignored_resources = Vec::new();
    let mut ignored_task_groups = Vec::new();
    collect_block(
        function,
        block,
        managed_resources,
        &mut ignored_resources,
        transfers,
        &mut ignored_task_groups,
    );
}

fn count_async_bindings(block: &Block) -> u32 {
    block
        .statements
        .iter()
        .filter(|statement| matches!(statement, Stmt::Let(binding) if binding.is_async))
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

fn count_select_arms(block: &Block) -> u32 {
    block
        .statements
        .iter()
        .filter_map(|statement| match statement {
            Stmt::Select(select) => Some(select.arms.len()),
            _ => None,
        })
        .sum::<usize>()
        .try_into()
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_syntax::parse_source;

    #[test]
    fn records_only_explicit_take_of_with_binding_as_resource_transfer() {
        let program = parse_source(
            "resource-transfer.rss",
            r#"
                fn main() -> Unit {
                    let ordinary = make_value()
                    consume(value: take ordinary)
                    with open_resource()? as file {
                        consume(value: take file)
                    }
                }
            "#,
        );
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };
        let mut resources = Vec::new();
        let mut transfers = Vec::new();
        let mut task_groups = Vec::new();
        collect_block(
            &function.name,
            &function.body,
            &BTreeSet::new(),
            &mut resources,
            &mut transfers,
            &mut task_groups,
        );

        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].function, "main");
        assert_eq!(transfers[0].binding, "file");
        assert_eq!(transfers[0].operation, "take");
    }
}
