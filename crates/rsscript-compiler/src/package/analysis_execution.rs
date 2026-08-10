use crate::syntax::ast::{Block, Item, Stmt};

use super::{
    PackageAnalysisResourceLifetime, PackageAnalysisTaskGroup, PackageReviewFileKind, PackageSource,
};

pub(super) fn collect_execution_facts(
    sources: &[PackageSource],
    database: &crate::semantic::SemanticDatabase,
) -> (
    Vec<PackageAnalysisResourceLifetime>,
    Vec<PackageAnalysisTaskGroup>,
) {
    let mut resources = Vec::new();
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
                    &mut resources,
                    &mut task_groups,
                );
            }
        }
    }
    resources.sort();
    resources.dedup();
    task_groups.sort();
    task_groups.dedup();
    (resources, task_groups)
}

fn collect_block(
    function: &str,
    block: &Block,
    resources: &mut Vec<PackageAnalysisResourceLifetime>,
    task_groups: &mut Vec<PackageAnalysisTaskGroup>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::With(with) => {
                resources.push(PackageAnalysisResourceLifetime {
                    function: function.to_string(),
                    binding: with.binding.clone(),
                    acquisition: "with".to_string(),
                    cleanup: "scope_exit".to_string(),
                    cleanup_on_cancellation: true,
                });
                collect_block(function, &with.body, resources, task_groups);
            }
            Stmt::TaskGroup(group) => {
                task_groups.push(PackageAnalysisTaskGroup {
                    function: function.to_string(),
                    spawned_tasks: count_async_bindings(&group.body),
                    select_arms: count_select_arms(&group.body),
                    drains_on_exit: true,
                    cleanup_on_cancellation: true,
                });
                collect_block(function, &group.body, resources, task_groups);
            }
            Stmt::If(statement) => {
                collect_block(function, &statement.then_body, resources, task_groups);
                if let Some(else_body) = &statement.else_body {
                    collect_block(function, else_body, resources, task_groups);
                }
            }
            Stmt::Loop(statement) => {
                collect_block(function, &statement.body, resources, task_groups)
            }
            Stmt::For(statement) => {
                collect_block(function, &statement.body, resources, task_groups)
            }
            Stmt::Match(statement) => {
                for arm in &statement.arms {
                    collect_block(function, &arm.body, resources, task_groups);
                }
            }
            Stmt::Select(statement) => {
                for arm in &statement.arms {
                    collect_block(function, &arm.body, resources, task_groups);
                }
            }
            Stmt::LetElse(statement) => {
                collect_block(function, &statement.else_body, resources, task_groups)
            }
            Stmt::Let(_)
            | Stmt::Return(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Assign(_)
            | Stmt::Expr(_)
            | Stmt::Unknown(_) => {}
        }
    }
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
