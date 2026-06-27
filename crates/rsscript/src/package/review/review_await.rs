use super::*;

pub(super) fn collect_package_await_sites(
    sources: &[PackageSource],
) -> Vec<PackageReviewAwaitSite> {
    let context = collect_await_site_context(sources);
    let mut await_sites = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .flat_map(|source| {
            let program = parse_source(&source.path, &source.contents);
            program
                .items
                .iter()
                .flat_map(|item| match item {
                    Item::Function(function) => {
                        collect_await_sites_in_block(&function.name, &function.body, &context)
                    }
                    Item::Type(type_decl) => {
                        type_decl.drop_body.as_ref().map_or_else(Vec::new, |body| {
                            collect_await_sites_in_block(
                                &format!("drop {}", type_decl.name),
                                body,
                                &context,
                            )
                        })
                    }
                    Item::Module(_)
                    | Item::Use(_)
                    | Item::SumType(_)
                    | Item::TypeAlias(_)
                    | Item::Const(_) => Vec::new(),
                })
                .map(|mut site| {
                    site.span.file = source.relative_path.clone();
                    site
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    await_sites.sort_by(|left, right| {
        left.span
            .file
            .cmp(&right.span.file)
            .then_with(|| left.span.line.cmp(&right.span.line))
            .then_with(|| left.span.column.cmp(&right.span.column))
            .then_with(|| left.function.cmp(&right.function))
    });
    await_sites
}

pub(super) struct AwaitSiteContext {
    async_native_callees: BTreeSet<String>,
    async_rss_callees: BTreeSet<String>,
}

pub(super) fn collect_await_site_context(sources: &[PackageSource]) -> AwaitSiteContext {
    let mut context = AwaitSiteContext {
        async_native_callees: BTreeSet::new(),
        async_rss_callees: BTreeSet::new(),
    };
    for source in sources {
        let program = parse_source(&source.path, &source.contents);
        for item in &program.items {
            let Item::Function(function) = item else {
                continue;
            };
            if !function.is_async {
                continue;
            }
            if function.is_native {
                context.async_native_callees.insert(function.name.clone());
            } else {
                context.async_rss_callees.insert(function.name.clone());
            }
        }
    }
    context
}

pub(super) fn collect_await_sites_in_block(
    function: &str,
    block: &Block,
    context: &AwaitSiteContext,
) -> Vec<PackageReviewAwaitSite> {
    let mut sites = Vec::new();
    collect_await_sites_from_block(
        function,
        block,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
        context,
        &mut sites,
    );
    sites
}

pub(super) fn collect_await_sites_from_stmt(
    function: &str,
    statement: &Stmt,
    live_after: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    pending_callees: &BTreeMap<String, String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_await_sites_from_expr(
                    function,
                    value,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_await_sites_from_expr(
                    function,
                    value,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::With(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.resource,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            let mut body_scoped_live = scoped_live.clone();
            body_scoped_live.insert(stmt.binding.clone());
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                &body_scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::If(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.condition,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_block(
                function,
                &stmt.then_body,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            if let Some(else_body) = &stmt.else_body {
                collect_await_sites_from_block(
                    function,
                    else_body,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_await_sites_from_expr(
                    function,
                    condition,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::For(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.iterable,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            let mut body_scoped_live = scoped_live.clone();
            body_scoped_live.insert(stmt.binding.clone());
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                &body_scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::TaskGroup(stmt) => {
            let mut task_group_pending_callees = pending_callees.clone();
            collect_task_group_async_let_callees(&stmt.body, &mut task_group_pending_callees);
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                scoped_live,
                &task_group_pending_callees,
                context,
                sites,
            );
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_await_sites_from_expr(
                    function,
                    &arm.operation,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                let mut arm_scoped_live = scoped_live.clone();
                if arm.binding != "_" {
                    arm_scoped_live.insert(arm.binding.clone());
                }
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    &arm_scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::Match(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            for arm in &stmt.arms {
                let mut arm_scoped_live = scoped_live.clone();
                for binding in arm.pattern.binding_names() {
                    arm_scoped_live.insert(binding.to_string());
                }
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    &arm_scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::LetElse(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_block(
                function,
                &stmt.else_body,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::Assign(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.target,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::Expr(expr) => collect_await_sites_from_expr(
            function,
            expr,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        ),
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

pub(super) fn collect_await_sites_from_block(
    function: &str,
    block: &Block,
    continuation_uses: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    pending_callees: &BTreeMap<String, String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    let live_after_statements = block_live_after_statements(block, continuation_uses);
    for (index, statement) in block.statements.iter().enumerate() {
        let live_after = live_after_statements
            .get(index)
            .unwrap_or(continuation_uses);
        collect_await_sites_from_stmt(
            function,
            statement,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        );
    }
}

pub(super) fn collect_await_sites_from_expr(
    function: &str,
    expr: &Expr,
    live_after: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    pending_callees: &BTreeMap<String, String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    match expr {
        Expr::Await { value, span } => {
            let mut live_across_await = scoped_live.clone();
            live_across_await.extend(live_after.iter().cloned());
            collect_expr_uses(value, &mut live_across_await);
            let callee = awaited_callee(value, pending_callees);
            sites.push(PackageReviewAwaitSite {
                function: function.to_string(),
                boundary: await_boundary(callee.as_deref(), context),
                callee,
                live_across_await: live_across_await.into_iter().collect(),
                span: span.clone(),
            });
            collect_await_sites_from_expr(
                function,
                value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => collect_await_sites_from_expr(
            function,
            value,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        ),
        Expr::Binary { left, right, .. } => {
            let mut left_live_after = live_after.clone();
            collect_expr_uses(right, &mut left_live_after);
            collect_await_sites_from_expr(
                function,
                left,
                &left_live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_expr(
                function,
                right,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Expr::Field { base, .. } => collect_await_sites_from_expr(
            function,
            base,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        ),
        Expr::Index { base, index, .. } => {
            let mut base_live_after = live_after.clone();
            collect_expr_uses(index, &mut base_live_after);
            collect_await_sites_from_expr(
                function,
                base,
                &base_live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_expr(
                function,
                index,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Expr::Call { args, .. } => {
            let mut arg_live_after = live_after.clone();
            for arg in args.iter().rev() {
                collect_await_sites_from_expr(
                    function,
                    &arg.value,
                    &arg_live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                collect_expr_uses(&arg.value, &mut arg_live_after);
            }
        }
        Expr::Closure { body, .. } => collect_await_sites_from_block(
            function,
            body,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            context,
            sites,
        ),
        Expr::Match { value, arms, .. } => {
            let mut value_live_after = live_after.clone();
            for arm in arms {
                collect_block_uses(&arm.body, &mut value_live_after);
            }
            collect_await_sites_from_expr(
                function,
                value,
                &value_live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            for arm in arms {
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Expr::MapLiteral { entries, .. } => {
            let mut entry_live_after = live_after.clone();
            for entry in entries.iter().rev() {
                collect_await_sites_from_expr(
                    function,
                    &entry.value,
                    &entry_live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                collect_expr_uses(&entry.value, &mut entry_live_after);
                collect_await_sites_from_expr(
                    function,
                    &entry.key,
                    &entry_live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                collect_expr_uses(&entry.key, &mut entry_live_after);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn block_live_after_statements(
    block: &Block,
    continuation_uses: &BTreeSet<String>,
) -> Vec<BTreeSet<String>> {
    let mut live_after = vec![BTreeSet::new(); block.statements.len()];
    let mut used = continuation_uses.clone();
    for (index, statement) in block.statements.iter().enumerate().rev() {
        live_after[index] = used.clone();
        collect_stmt_uses(statement, &mut used);
        remove_stmt_bindings(statement, &mut used);
    }
    live_after
}

pub(super) fn collect_task_group_async_let_callees(
    block: &Block,
    pending_callees: &mut BTreeMap<String, String>,
) {
    for statement in &block.statements {
        if let Stmt::Let(stmt) = statement
            && stmt.is_async
            && let Some(value) = &stmt.value
            && let Some(callee) = awaited_callee(value, pending_callees)
        {
            pending_callees.insert(stmt.name.clone(), callee);
        }
    }
}

pub(super) fn collect_stmt_uses(statement: &Stmt, uses: &mut BTreeSet<String>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_uses(value, uses);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_uses(value, uses);
            }
        }
        Stmt::With(stmt) => {
            collect_expr_uses(&stmt.resource, uses);
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::If(stmt) => {
            collect_expr_uses(&stmt.condition, uses);
            collect_block_uses(&stmt.then_body, uses);
            if let Some(else_body) = &stmt.else_body {
                collect_block_uses(else_body, uses);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_expr_uses(condition, uses);
            }
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::For(stmt) => {
            collect_expr_uses(&stmt.iterable, uses);
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::TaskGroup(stmt) => {
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_expr_uses(&arm.operation, uses);
                collect_block_uses(&arm.body, uses);
            }
        }
        Stmt::Match(stmt) => {
            collect_expr_uses(&stmt.value, uses);
            for arm in &stmt.arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_expr_uses(&stmt.value, uses);
            collect_block_uses(&stmt.else_body, uses);
        }
        Stmt::Assign(stmt) => {
            collect_expr_uses(&stmt.target, uses);
            collect_expr_uses(&stmt.value, uses);
        }
        Stmt::Expr(expr) => collect_expr_uses(expr, uses),
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

pub(super) fn collect_block_uses(block: &Block, uses: &mut BTreeSet<String>) {
    let mut block_uses = BTreeSet::new();
    for statement in block.statements.iter().rev() {
        collect_stmt_uses(statement, &mut block_uses);
        remove_stmt_bindings(statement, &mut block_uses);
    }
    uses.extend(block_uses);
}

pub(super) fn collect_expr_uses(expr: &Expr, uses: &mut BTreeSet<String>) {
    match expr {
        Expr::Ident(name, _) => {
            if !is_builtin_value_ident(name) {
                uses.insert(name.clone());
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_uses(left, uses);
            collect_expr_uses(right, uses);
        }
        Expr::Field { base, .. } => collect_expr_uses(base, uses),
        Expr::Index { base, index, .. } => {
            collect_expr_uses(base, uses);
            collect_expr_uses(index, uses);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_uses(&arg.value, uses);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => collect_expr_uses(value, uses),
        Expr::Closure { body, .. } => collect_block_uses(body, uses),
        Expr::Match { value, arms, .. } => {
            collect_expr_uses(value, uses);
            for arm in arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_uses(&entry.key, uses);
                collect_expr_uses(&entry.value, uses);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn is_builtin_value_ident(name: &str) -> bool {
    matches!(name, "Unit" | "true" | "false")
}

pub(super) fn await_boundary(
    callee: Option<&str>,
    context: &AwaitSiteContext,
) -> PackageReviewAwaitBoundary {
    let Some(callee) = callee else {
        return PackageReviewAwaitBoundary::Unknown;
    };
    if runtime_intrinsic_label(callee) {
        return PackageReviewAwaitBoundary::RuntimePending;
    }
    if context.async_native_callees.contains(callee) {
        return PackageReviewAwaitBoundary::NativePending;
    }
    if context.async_rss_callees.contains(callee) {
        return PackageReviewAwaitBoundary::RssCall;
    }
    PackageReviewAwaitBoundary::Unknown
}

pub(super) fn runtime_intrinsic_label(callee: &str) -> bool {
    let Some((namespace, name)) = callee.rsplit_once('.') else {
        return false;
    };
    runtime_abi::lookup_runtime_intrinsic(namespace, name).is_some()
}

pub(super) fn remove_stmt_bindings(statement: &Stmt, uses: &mut BTreeSet<String>) {
    match statement {
        Stmt::Let(stmt) => {
            uses.remove(&stmt.name);
        }
        Stmt::With(stmt) => {
            uses.remove(&stmt.binding);
        }
        Stmt::For(stmt) => {
            uses.remove(&stmt.binding);
        }
        Stmt::TaskGroup(_) => {}
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                if arm.binding != "_" {
                    uses.remove(&arm.binding);
                }
            }
        }
        Stmt::Match(stmt) => {
            for arm in &stmt.arms {
                for binding in arm.pattern.binding_names() {
                    uses.remove(binding);
                }
            }
        }
        Stmt::LetElse(stmt) => {
            if !stmt.binding_name.is_empty() {
                uses.remove(&stmt.binding_name);
            }
        }
        Stmt::Assign(_)
        | Stmt::Return(_)
        | Stmt::If(_)
        | Stmt::Loop(_)
        | Stmt::Expr(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

pub(super) fn awaited_callee(
    expr: &Expr,
    pending_callees: &BTreeMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Call { callee, .. } => Some(callee_label(callee)),
        Expr::Ident(name, _) => pending_callees.get(name).cloned(),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            awaited_callee(value, pending_callees)
        }
        _ => None,
    }
}
