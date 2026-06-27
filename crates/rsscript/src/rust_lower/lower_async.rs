use crate::syntax::ast::{Block, DataEffect, Expr, Stmt};

use super::helpers::*;

use super::lowerer::*;

impl RustLowerer<'_> {
    pub(super) fn lower_task_group_block(
        &mut self,
        block: &Block,
        out: &mut String,
        indent: usize,
    ) {
        let pad = "    ".repeat(indent);
        let executor = self
            .current_async_executor
            .clone()
            .unwrap_or_else(|| "__rsscript_async_executor".to_string());

        // The group owns a cancellation guard. It is declared first so it drops
        // last: on any scope exit (normal, early `return`, or `?` error) it
        // cancels the group token, stopping cooperative siblings.
        let guard = "_rsscript_task_group".to_string();
        out.push_str(&format!(
            "{pad}let {guard} = rsscript_runtime::TaskGroupScope::new();\n"
        ));
        let previous_task_group_token = self.current_task_group_token.replace(guard);

        let async_let_names: Vec<String> = block
            .statements
            .iter()
            .filter_map(|statement| match statement {
                Stmt::Let(stmt) if stmt.is_async && stmt.name != "_" => {
                    Some(rust_ident(&stmt.name))
                }
                _ => None,
            })
            .collect();

        out.push_str(&format!(
            "{pad}let __rsscript_task_group_trace_started = std::time::Instant::now();\n"
        ));
        out.push_str(&format!(
            "{pad}let __rsscript_task_group_trace_polls = {executor}.poll_count();\n"
        ));
        out.push_str(&format!(
            "{pad}let __rsscript_task_group_trace_yields = {executor}.yield_count();\n"
        ));
        out.push_str(&format!(
            "{pad}let __rsscript_task_group_trace_tasks = {}usize;\n",
            async_let_names.len()
        ));

        // Lower statements in source order so an `async let` can reference an
        // earlier binding. `async let x = f()` constructs (does not run) the
        // pending; `await x` drives every still-running sibling until x is ready,
        // then reads x and applies `?` — so an error propagates immediately (the
        // guard drops and cancels the rest) and a never-completing sibling never
        // blocks an unrelated await.
        //
        // `active` is the set of async-lets that have been *declared* but not yet
        // awaited. An await only polls this set, so it never forward-references a
        // pending declared later, and never re-polls a task whose result was
        // already taken (`take` resets the result slot to `None`).
        let mut active: Vec<String> = Vec::new();
        for (statement_index, statement) in block.statements.iter().enumerate() {
            match statement {
                Stmt::Let(stmt) if stmt.is_async => {
                    if let Some(value) = &stmt.value {
                        let name = if stmt.name == "_" {
                            format!("discard_{statement_index}")
                        } else {
                            rust_ident(&stmt.name)
                        };
                        let lowered = self.lower_async_let_pending(value, out, &pad, &name);
                        out.push_str(&format!(
                            "{pad}let __rsscript_key_{name} = {statement_index}usize;\n"
                        ));
                        out.push_str(&format!(
                            "{pad}let mut __rsscript_pending_{name} = {lowered};\n"
                        ));
                        out.push_str(&format!("{pad}let mut __rsscript_result_{name} = None;\n"));
                        active.push(name);
                    }
                }
                Stmt::Let(stmt)
                    if stmt
                        .value
                        .as_ref()
                        .and_then(|value| self.extract_task_group_await(value, &async_let_names))
                        .is_some() =>
                {
                    let value = stmt.value.as_ref().expect("await let value");
                    let pending = self
                        .extract_task_group_await(value, &async_let_names)
                        .expect("await handle");
                    let try_suffix = if is_try_wrapped(value) { "?" } else { "" };
                    let mutable = if self.mutated_bindings.contains(&stmt.name) {
                        "mut "
                    } else {
                        ""
                    };
                    self.emit_task_group_poll_until(out, &pad, &executor, &active, &pending);
                    active.retain(|name| name != &pending);
                    out.push_str(&format!(
                        "{pad}let {mutable}{} = __rsscript_result_{pending}.take().expect(\"awaited task completed\"){try_suffix};\n",
                        rust_ident(&stmt.name),
                    ));
                }
                Stmt::Expr(expr)
                    if self
                        .extract_task_group_await(expr, &async_let_names)
                        .is_some() =>
                {
                    let pending = self
                        .extract_task_group_await(expr, &async_let_names)
                        .expect("await handle");
                    let try_suffix = if is_try_wrapped(expr) { "?" } else { "" };
                    self.emit_task_group_poll_until(out, &pad, &executor, &active, &pending);
                    active.retain(|name| name != &pending);
                    out.push_str(&format!(
                        "{pad}__rsscript_result_{pending}.take().expect(\"awaited task completed\"){try_suffix};\n"
                    ));
                }
                _ => self.lower_stmt(statement, out, indent),
            }
        }

        if !active.is_empty() {
            self.emit_task_group_drain(out, &pad, &executor, &active);
        }

        out.push_str(&format!(
            "{pad}rsscript_runtime::trace_async_runtime_phase(\n"
        ));
        out.push_str(&format!("{pad}    \"task_group_scope\",\n"));
        out.push_str(&format!(
            "{pad}    __rsscript_task_group_trace_started.elapsed().as_micros(),\n"
        ));
        out.push_str(&format!(
            "{pad}    {executor}.poll_count().saturating_sub(__rsscript_task_group_trace_polls),\n"
        ));
        out.push_str(&format!(
            "{pad}    {executor}.yield_count().saturating_sub(__rsscript_task_group_trace_yields),\n"
        ));
        out.push_str(&format!("{pad}    __rsscript_task_group_trace_tasks,\n"));
        out.push_str(&format!("{pad});\n"));

        self.current_task_group_token = previous_task_group_token;
    }

    pub(super) fn lower_async_let_pending(
        &mut self,
        expr: &Expr,
        out: &mut String,
        pad: &str,
        pending_name: &str,
    ) -> String {
        let Expr::Call { callee, args, span } = expr else {
            return self.lower_expr(expr);
        };

        let mut rewritten_args = args.clone();
        let mut temp_index = 0usize;
        for (arg_index, arg) in rewritten_args.iter_mut().enumerate() {
            let Expr::Effect {
                effect: DataEffect::Read,
                value,
                span: effect_span,
            } = &arg.value
            else {
                continue;
            };
            if Self::expr_is_stable_borrow_place(value) {
                continue;
            }

            let temp_name = format!("__rsscript_async_arg_{pending_name}_{temp_index}");
            temp_index += 1;
            let lowered_value =
                if let Some(expected) = self.expected_call_arg_type(callee, arg, arg_index) {
                    self.lower_expr_for_expected_type(value, &expected)
                } else {
                    self.lower_owned_expr(value)
                };
            out.push_str(&format!("{pad}let {temp_name} = {lowered_value};\n"));
            arg.value = Expr::Effect {
                effect: DataEffect::Read,
                value: Box::new(Expr::Ident(temp_name, effect_span.clone())),
                span: effect_span.clone(),
            };
        }

        let rewritten = Expr::Call {
            callee: callee.clone(),
            args: rewritten_args,
            span: span.clone(),
        };
        self.lower_expr(&rewritten)
    }

    pub(super) fn expr_is_stable_borrow_place(expr: &Expr) -> bool {
        match expr {
            Expr::Ident(..) => true,
            Expr::Field { base, .. } | Expr::Index { base, .. } => {
                Self::expr_is_stable_borrow_place(base)
            }
            _ => false,
        }
    }

    /// Emit a cooperative poll loop that drives every currently-active async-let
    /// sibling until `target`'s result is ready (so siblings interleave, but the
    /// await only waits for its own task). `active` holds only async-lets already
    /// declared and not yet awaited, so the loop never references a pending that
    /// is declared later or whose result was already taken.
    pub(super) fn emit_task_group_poll_until(
        &self,
        out: &mut String,
        pad: &str,
        executor: &str,
        active: &[String],
        target: &str,
    ) {
        out.push_str(&format!(
            "{pad}if __rsscript_result_{target}.is_none() {{\n"
        ));
        out.push_str(&format!(
            "{pad}    let mut __rsscript_ready_keys: Option<Vec<usize>> = None;\n"
        ));
        out.push_str(&format!("{pad}    loop {{\n"));
        for name in active {
            out.push_str(&format!(
                "{pad}        if __rsscript_result_{name}.is_none() {{\n"
            ));
            out.push_str(&format!(
                "{pad}            let __rsscript_should_poll = __rsscript_ready_keys.as_ref().map_or(true, |__rsscript_keys| __rsscript_keys.contains(&__rsscript_key_{name}));\n"
            ));
            out.push_str(&format!("{pad}            if __rsscript_should_poll {{\n"));
            out.push_str(&format!(
                "{pad}                if let rsscript_runtime::AsyncPoll::Ready(__rsscript_value) = {executor}.poll_once_keyed(__rsscript_key_{name}, &mut __rsscript_pending_{name}) {{\n"
            ));
            out.push_str(&format!(
                "{pad}                    __rsscript_result_{name} = Some(__rsscript_value);\n"
            ));
            out.push_str(&format!("{pad}                }}\n"));
            out.push_str(&format!("{pad}            }}\n"));
            out.push_str(&format!("{pad}        }}\n"));
        }
        out.push_str(&format!(
            "{pad}        if __rsscript_result_{target}.is_some() {{ break; }}\n"
        ));
        out.push_str(&format!("{pad}        {executor}.yield_once();\n"));
        out.push_str(&format!(
            "{pad}        let __rsscript_woken_keys = {executor}.drain_ready_wake_keys();\n"
        ));
        out.push_str(&format!(
            "{pad}        __rsscript_ready_keys = if __rsscript_woken_keys.is_empty() {{ None }} else {{ Some(__rsscript_woken_keys) }};\n"
        ));
        out.push_str(&format!("{pad}    }}\n"));
        out.push_str(&format!("{pad}}}\n"));
    }

    pub(super) fn emit_task_group_drain(
        &self,
        out: &mut String,
        pad: &str,
        executor: &str,
        active: &[String],
    ) {
        out.push_str(&format!(
            "{pad}let mut __rsscript_ready_keys: Option<Vec<usize>> = None;\n"
        ));
        out.push_str(&format!("{pad}loop {{\n"));
        out.push_str(&format!("{pad}    let mut __rsscript_all_done = true;\n"));
        for name in active {
            out.push_str(&format!(
                "{pad}    if __rsscript_result_{name}.is_none() {{\n"
            ));
            out.push_str(&format!("{pad}        __rsscript_all_done = false;\n"));
            out.push_str(&format!(
                "{pad}        let __rsscript_should_poll = __rsscript_ready_keys.as_ref().map_or(true, |__rsscript_keys| __rsscript_keys.contains(&__rsscript_key_{name}));\n"
            ));
            out.push_str(&format!("{pad}        if __rsscript_should_poll {{\n"));
            out.push_str(&format!(
                "{pad}            if let rsscript_runtime::AsyncPoll::Ready(__rsscript_value) = {executor}.poll_once_keyed(__rsscript_key_{name}, &mut __rsscript_pending_{name}) {{\n"
            ));
            out.push_str(&format!(
                "{pad}                __rsscript_result_{name} = Some(__rsscript_value);\n"
            ));
            out.push_str(&format!("{pad}            }}\n"));
            out.push_str(&format!("{pad}        }}\n"));
            out.push_str(&format!("{pad}    }}\n"));
        }
        out.push_str(&format!("{pad}    if __rsscript_all_done {{ break; }}\n"));
        out.push_str(&format!("{pad}    {executor}.yield_once();\n"));
        out.push_str(&format!(
            "{pad}    let __rsscript_woken_keys = {executor}.drain_ready_wake_keys();\n"
        ));
        out.push_str(&format!(
            "{pad}    __rsscript_ready_keys = if __rsscript_woken_keys.is_empty() {{ None }} else {{ Some(__rsscript_woken_keys) }};\n"
        ));
        out.push_str(&format!("{pad}}}\n"));
    }

    pub(super) fn lower_select_stmt(
        &mut self,
        stmt: &crate::syntax::ast::SelectStmt,
        out: &mut String,
        indent: usize,
    ) {
        let pad = "    ".repeat(indent);
        let inner_pad = "    ".repeat(indent + 1);
        let executor = self
            .current_async_executor
            .clone()
            .unwrap_or_else(|| "__rsscript_async_executor".to_string());
        let prefix = format!("__rsscript_select_{}_{}", stmt.span.line, stmt.span.column);

        out.push_str(&format!("{pad}{{\n"));
        for (index, arm) in stmt.arms.iter().enumerate() {
            let pending = async_await_inner(&arm.operation)
                .map(|inner| self.lower_expr(inner))
                .unwrap_or_else(|| unreachable_lowering("select arm operation", &arm.span));
            out.push_str(&format!(
                "{inner_pad}let mut {prefix}_pending_{index} = {pending};\n"
            ));
            out.push_str(&format!(
                "{inner_pad}let mut {prefix}_result_{index} = None;\n"
            ));
        }

        out.push_str(&format!("{inner_pad}let {prefix}_arm = loop {{\n"));
        for index in 0..stmt.arms.len() {
            out.push_str(&format!(
                "{inner_pad}    if {prefix}_result_{index}.is_none() {{\n"
            ));
            out.push_str(&format!(
                "{inner_pad}        if let rsscript_runtime::AsyncPoll::Ready(__rsscript_value) = {executor}.poll_once(&mut {prefix}_pending_{index}) {{\n"
            ));
            out.push_str(&format!(
                "{inner_pad}            {prefix}_result_{index} = Some(__rsscript_value);\n"
            ));
            out.push_str(&format!("{inner_pad}            break {index};\n"));
            out.push_str(&format!("{inner_pad}        }}\n"));
            out.push_str(&format!("{inner_pad}    }}\n"));
        }
        out.push_str(&format!("{inner_pad}    {executor}.yield_once();\n"));
        out.push_str(&format!("{inner_pad}}};\n"));

        out.push_str(&format!("{inner_pad}match {prefix}_arm {{\n"));
        for (index, arm) in stmt.arms.iter().enumerate() {
            out.push_str(&format!("{inner_pad}    {index} => {{\n"));
            let take_result =
                format!("{prefix}_result_{index}.take().expect(\"select arm completed\")");
            if arm.binding == "_" {
                if async_await_is_try(&arm.operation) {
                    out.push_str(&format!("{inner_pad}        {take_result}?;\n"));
                } else {
                    out.push_str(&format!("{inner_pad}        let _ = {take_result};\n"));
                }
            } else {
                let try_suffix = if async_await_is_try(&arm.operation) {
                    "?"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "{inner_pad}        let {} = {take_result}{try_suffix};\n",
                    rust_ident(&arm.binding)
                ));
            }
            self.lower_block(&arm.body, out, indent + 2);
            out.push_str(&format!("{inner_pad}    }}\n"));
        }
        out.push_str(&format!("{inner_pad}    _ => unreachable!(),\n"));
        out.push_str(&format!("{inner_pad}}}\n"));
        out.push_str(&format!("{pad}}}\n"));
    }

    pub(super) fn extract_task_group_await<'b>(
        &self,
        expr: &'b Expr,
        async_let_names: &[String],
    ) -> Option<String> {
        match expr {
            Expr::Await { value, .. } => match value.as_ref() {
                Expr::Ident(name, _) if async_let_names.contains(name) => Some(rust_ident(name)),
                Expr::Effect { value, .. } => {
                    // await read x / await take x
                    if let Expr::Ident(name, _) = value.as_ref() {
                        if async_let_names.contains(name) {
                            return Some(rust_ident(name));
                        }
                    }
                    None
                }
                _ => None,
            },
            Expr::Try { value, .. } => self.extract_task_group_await(value, async_let_names),
            _ => None,
        }
    }

    /// Lower a linear `async fn` body into a `pending_try`/`pending_then`/
    /// `pending_ready` chain — a single `impl Pending<Ret>` expression. Each
    /// `await op()?` becomes `pending_try(op(), move |x| <rest>)`; non-await
    /// statements run inside the continuation block; `return e` is `pending_ready(e)`.
    pub(super) fn lower_async_chain(&mut self, statements: &[Stmt]) -> String {
        let Some((head, tail)) = statements.split_first() else {
            return "rsscript_runtime::pending_ready(())".to_string();
        };
        match head {
            // `return await op` / `return await op?`.
            Stmt::Return(stmt)
                if stmt
                    .value
                    .as_ref()
                    .and_then(|value| async_await_inner(value))
                    .is_some() =>
            {
                let value = stmt.value.as_ref().expect("return value");
                let pending = self.lower_expr(async_await_inner(value).expect("await inner"));
                if async_await_is_try(value) {
                    // `return await op?`: drive op (a `Result`-pending), short-circuit
                    // on `Err`, and re-wrap the `Ok` value as the function's pending so
                    // the `?` propagation is preserved.
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |__rsscript_return_value| {{ rsscript_runtime::pending_ready(Ok(__rsscript_return_value)) }})"
                    )
                } else {
                    // `return await op`: the function's pending IS op's pending.
                    pending
                }
            }
            Stmt::Return(stmt) => {
                // Reuse the sync return machinery so a `read` Copy param is deref'd
                // and a bare value is `Ok`-wrapped to match the declared return type
                // (the pending must resolve to the function's return type, not `&T`).
                let value = stmt
                    .value
                    .as_ref()
                    .map(|value| self.lower_return_expr(value))
                    .unwrap_or_else(|| "()".to_string());
                format!("rsscript_runtime::pending_ready({value})")
            }
            Stmt::Let(stmt)
                if stmt
                    .value
                    .as_ref()
                    .and_then(|value| async_await_inner(value))
                    .is_some() =>
            {
                let value = stmt.value.as_ref().expect("await let has a value");
                let combinator = if async_await_is_try(value) {
                    "pending_try"
                } else {
                    "pending_then"
                };
                let pending = self.lower_expr(async_await_inner(value).expect("await inner"));
                let bind = closure_binding(&stmt.name, stmt.is_mut);
                let binding_ty = awaited_binding_type(
                    self.infer_expr_type(async_await_inner(value).expect("await inner")),
                    async_await_is_try(value),
                );
                let previous_binding_ty = binding_ty
                    .clone()
                    .and_then(|ty| self.value_types.insert(stmt.name.clone(), ty));
                let rest = self.lower_async_chain(tail);
                if binding_ty.is_some() {
                    if let Some(previous) = previous_binding_ty {
                        self.value_types.insert(stmt.name.clone(), previous);
                    } else {
                        self.value_types.remove(&stmt.name);
                    }
                }
                format!("rsscript_runtime::{combinator}({pending}, move |{bind}| {{ {rest} }})")
            }
            Stmt::Expr(expr) if async_await_inner(expr).is_some() => {
                let combinator = if async_await_is_try(expr) {
                    "pending_try"
                } else {
                    "pending_then"
                };
                let pending = self.lower_expr(async_await_inner(expr).expect("await inner"));
                let rest = self.lower_async_chain(tail);
                format!(
                    "rsscript_runtime::{combinator}({pending}, move |_rsscript_unit| {{ {rest} }})"
                )
            }
            Stmt::Let(stmt)
                if stmt.value.as_ref().is_some_and(|value| {
                    async_await_inner(value).is_none() && is_try_wrapped(value)
                }) =>
            {
                let value = stmt.value.as_ref().expect("try let has a value");
                let Expr::Try { value: inner, .. } = value else {
                    unreachable!("is_try_wrapped matched a non-try expression")
                };
                let result_ty = self.infer_expr_type(inner);
                let binding_ty = result_ty.as_ref().and_then(result_ok_type_ref);
                let pending = if let (Some(ok_ty), Some(error_ty)) =
                    (binding_ty.as_ref(), self.current_result_error_type_rust())
                {
                    let ok_ty = self.lower_type_ref(ok_ty, ManagedPosition::Return);
                    let inner = self.lower_expr(inner);
                    format!(
                        "rsscript_runtime::pending_ready((|| -> Result<{ok_ty}, {error_ty}> {{ Ok({inner}?) }})())"
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_ready({})",
                        self.lower_expr(inner)
                    )
                };
                let bind = closure_binding(&stmt.name, stmt.is_mut);
                let previous_binding_ty = binding_ty
                    .clone()
                    .and_then(|ty| self.value_types.insert(stmt.name.clone(), ty));
                let rest = self.lower_async_chain(tail);
                if binding_ty.is_some() {
                    if let Some(previous) = previous_binding_ty {
                        self.value_types.insert(stmt.name.clone(), previous);
                    } else {
                        self.value_types.remove(&stmt.name);
                    }
                }
                format!("rsscript_runtime::pending_try({pending}, move |{bind}| {{ {rest} }})")
            }
            Stmt::Expr(expr) if is_try_wrapped(expr) => {
                let Expr::Try { value: inner, .. } = expr else {
                    unreachable!("is_try_wrapped matched a non-try expression")
                };
                let pending = if let Some(error_ty) = self.current_result_error_type_rust() {
                    let inner = self.lower_expr(inner);
                    format!(
                        "rsscript_runtime::pending_ready((|| -> Result<(), {error_ty}> {{ {inner}?; Ok(()) }})())"
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_ready({})",
                        self.lower_expr(inner)
                    )
                };
                let rest = self.lower_async_chain(tail);
                format!(
                    "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})"
                )
            }
            Stmt::TaskGroup(stmt) => {
                let boundary = self.lower_async_task_group_boundary(&stmt.body);
                let rest = self.lower_async_chain(tail);
                if boundary.returns_result {
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_then({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                }
            }
            Stmt::Select(_) => {
                let boundary = self.lower_async_statement_boundary(head);
                let rest = self.lower_async_chain(tail);
                if boundary.returns_result {
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_then({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                }
            }
            Stmt::For(stmt) if stmt.is_async => {
                let boundary = self.lower_async_statement_boundary(head);
                let rest = self.lower_async_chain(tail);
                if boundary.returns_result {
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_then({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                }
            }
            Stmt::Loop(stmt) if stmt_contains_await(head) => {
                let boundary = self.lower_async_loop_boundary(stmt);
                let rest = self.lower_async_chain(tail);
                if boundary.returns_result {
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_then({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                }
            }
            Stmt::If(_) | Stmt::Match(_) | Stmt::With(_) if stmt_contains_await(head) => {
                let boundary = self.lower_async_statement_boundary(head);
                let rest = self.lower_async_chain(tail);
                if boundary.returns_result {
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_then({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                }
            }
            other if stmt_contains_try(other) => {
                let boundary = self.lower_async_statement_boundary(other);
                let rest = self.lower_async_chain(tail);
                if boundary.returns_result {
                    format!(
                        "rsscript_runtime::pending_try({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                } else {
                    format!(
                        "rsscript_runtime::pending_then({pending}, move |_rsscript_unit| {{ {rest} }})",
                        pending = boundary.pending
                    )
                }
            }
            other => {
                let mut statement_out = String::new();
                self.lower_stmt(other, &mut statement_out, 0);
                let rest = self.lower_async_chain(tail);
                format!("{{ {} {rest} }}", statement_out.trim())
            }
        }
    }

    pub(super) fn lower_async_statement_boundary(
        &mut self,
        statement: &Stmt,
    ) -> AsyncTaskGroupBoundary {
        let mut body = String::new();
        let previous_executor = self
            .current_async_executor
            .replace("__rsscript_async_executor".to_string());
        body.push_str("{\n");
        body.push_str("let mut __rsscript_async_executor = rsscript_runtime::Executor::new();\n");
        self.lower_stmt(statement, &mut body, 0);
        let returns_result = if let Some(error_ty) = self.current_result_error_type_rust() {
            body.push_str(&format!("Ok::<(), {error_ty}>(())\n"));
            true
        } else {
            body.push_str("()\n");
            false
        };
        body.push('}');
        self.current_async_executor = previous_executor;
        AsyncTaskGroupBoundary {
            pending: format!("rsscript_runtime::pending_defer(move || {body})"),
            returns_result,
        }
    }

    pub(super) fn lower_async_task_group_boundary(
        &mut self,
        block: &Block,
    ) -> AsyncTaskGroupBoundary {
        let mut body = String::new();
        let previous_executor = self
            .current_async_executor
            .replace("__rsscript_async_executor".to_string());
        body.push_str("{\n");
        body.push_str("let mut __rsscript_async_executor = rsscript_runtime::Executor::new();\n");
        body.push_str("// task_group: structured concurrency scope\n");
        body.push_str("{\n");
        self.lower_task_group_block(block, &mut body, 1);
        body.push_str("}\n");
        let returns_result = if let Some(error_ty) = self.current_result_error_type_rust() {
            body.push_str(&format!("Ok::<(), {error_ty}>(())\n"));
            true
        } else {
            body.push_str("()\n");
            false
        };
        body.push('}');
        self.current_async_executor = previous_executor;
        AsyncTaskGroupBoundary {
            pending: format!("rsscript_runtime::pending_defer(move || {body})"),
            returns_result,
        }
    }

    pub(super) fn lower_async_loop_boundary(
        &mut self,
        stmt: &crate::syntax::ast::LoopStmt,
    ) -> AsyncTaskGroupBoundary {
        let Some(error_ty) = self.current_result_error_type_rust() else {
            return self.lower_async_statement_boundary(&Stmt::Loop(stmt.clone()));
        };
        if let Some(boundary) = self.lower_async_poll_loop_boundary(stmt, &error_ty) {
            return boundary;
        }
        let condition = stmt
            .condition
            .as_ref()
            .map(|condition| self.lower_expr(condition))
            .unwrap_or_else(|| "true".to_string());
        let body = self.lower_async_loop_body_chain(&stmt.body.statements, &error_ty);
        AsyncTaskGroupBoundary {
            pending: format!(
                "rsscript_runtime::pending_loop_result::<{error_ty}, _>(move || {{
                    if !({condition}) {{
                        Box::new(rsscript_runtime::pending_ready(Ok::<rsscript_runtime::LoopControl, {error_ty}>(rsscript_runtime::LoopControl::Break))) as Box<dyn rsscript_runtime::Pending<Result<rsscript_runtime::LoopControl, {error_ty}>> + '_>
                    }} else {{
                        Box::new({body}) as Box<dyn rsscript_runtime::Pending<Result<rsscript_runtime::LoopControl, {error_ty}>> + '_>
                    }}
                }})"
            ),
            returns_result: true,
        }
    }

    pub(super) fn lower_async_poll_loop_boundary(
        &mut self,
        stmt: &crate::syntax::ast::LoopStmt,
        error_ty: &str,
    ) -> Option<AsyncTaskGroupBoundary> {
        let (before_await, await_stmt, rest) =
            split_loop_body_at_first_await(&stmt.body.statements)?;
        if rest.iter().any(stmt_contains_await) {
            return None;
        }
        let condition = stmt
            .condition
            .as_ref()
            .map(|condition| self.lower_expr(condition))
            .unwrap_or_else(|| "true".to_string());
        let prefix = format!("__rsscript_loop_{}_{}", stmt.span.line, stmt.span.column);
        let (pending_expr, binding, is_try) = match await_stmt {
            Stmt::Let(let_stmt) => {
                let value = let_stmt.value.as_ref()?;
                let inner = async_await_inner(value)?;
                let binding_ty =
                    awaited_binding_type(self.infer_expr_type(inner), async_await_is_try(value));
                (
                    self.lower_expr(inner),
                    Some((
                        let_stmt.name.clone(),
                        rust_ident(&let_stmt.name),
                        binding_ty,
                    )),
                    async_await_is_try(value),
                )
            }
            Stmt::Expr(expr) => {
                let inner = async_await_inner(expr)?;
                (self.lower_expr(inner), None, async_await_is_try(expr))
            }
            _ => return None,
        };
        let mut before_body = String::new();
        for statement in before_await {
            self.lower_stmt(statement, &mut before_body, 7);
        }
        let previous_binding_ty = if let Some((name, _, Some(ty))) = binding.as_ref() {
            self.value_types.insert(name.clone(), ty.clone())
        } else {
            None
        };
        let mut rest_body = String::new();
        for statement in rest {
            self.lower_stmt(statement, &mut rest_body, 4);
        }
        if let Some((name, _, Some(_))) = binding.as_ref() {
            if let Some(previous) = previous_binding_ty {
                self.value_types.insert(name.clone(), previous);
            } else {
                self.value_types.remove(name);
            }
        }
        let value_bind = match (binding, is_try) {
            (Some((_, binding, _)), true) => format!(
                "let {binding} = match {prefix}_value {{
                    Ok(__rsscript_ok) => __rsscript_ok,
                    Err(__rsscript_error) => return rsscript_runtime::AsyncPoll::Ready(Err(__rsscript_error)),
                }};"
            ),
            (Some((_, binding, _)), false) => format!("let {binding} = {prefix}_value;"),
            (None, true) => format!(
                "if let Err(__rsscript_error) = {prefix}_value {{
                    return rsscript_runtime::AsyncPoll::Ready(Err(__rsscript_error));
                }}"
            ),
            (None, false) => format!("let _ = {prefix}_value;"),
        };
        let pending = format!(
            "{{
                let mut {prefix}_pending = None;
                rsscript_runtime::pending_poll_fn(move |{prefix}_cx| {{
                    loop {{
                        if !({condition}) {{
                            return rsscript_runtime::AsyncPoll::Ready(Ok::<(), {error_ty}>(()));
                        }}
                        if {prefix}_pending.is_none() {{
{before_body}
                            {prefix}_pending = Some({pending_expr});
                        }}
                        match rsscript_runtime::Pending::poll(
                            {prefix}_pending.as_mut().expect(\"loop await pending exists\"),
                            {prefix}_cx,
                        ) {{
                            rsscript_runtime::AsyncPoll::Ready({prefix}_value) => {{
                                {prefix}_pending = None;
                                {value_bind}
{rest_body}
                            }}
                            rsscript_runtime::AsyncPoll::Pending => {{
                                return rsscript_runtime::AsyncPoll::Pending;
                            }}
                        }}
                    }}
                }})
            }}"
        );
        Some(AsyncTaskGroupBoundary {
            pending,
            returns_result: true,
        })
    }

    pub(super) fn lower_async_loop_body_chain(
        &mut self,
        statements: &[Stmt],
        error_ty: &str,
    ) -> String {
        let Some((head, tail)) = statements.split_first() else {
            return format!(
                "rsscript_runtime::pending_ready(Ok::<rsscript_runtime::LoopControl, {error_ty}>(rsscript_runtime::LoopControl::Continue))"
            );
        };
        match head {
            Stmt::Break(_) => format!(
                "rsscript_runtime::pending_ready(Ok::<rsscript_runtime::LoopControl, {error_ty}>(rsscript_runtime::LoopControl::Break))"
            ),
            Stmt::Continue(_) => format!(
                "rsscript_runtime::pending_ready(Ok::<rsscript_runtime::LoopControl, {error_ty}>(rsscript_runtime::LoopControl::Continue))"
            ),
            Stmt::Let(stmt)
                if stmt
                    .value
                    .as_ref()
                    .and_then(|value| async_await_inner(value))
                    .is_some() =>
            {
                let value = stmt.value.as_ref().expect("await let has a value");
                let combinator = if async_await_is_try(value) {
                    "pending_try"
                } else {
                    "pending_then"
                };
                let pending = self.lower_expr(async_await_inner(value).expect("await inner"));
                let bind = closure_binding(&stmt.name, stmt.is_mut);
                let binding_ty = awaited_binding_type(
                    self.infer_expr_type(async_await_inner(value).expect("await inner")),
                    async_await_is_try(value),
                );
                let previous_binding_ty = binding_ty
                    .clone()
                    .and_then(|ty| self.value_types.insert(stmt.name.clone(), ty));
                let rest = self.lower_async_loop_body_chain(tail, error_ty);
                if binding_ty.is_some() {
                    if let Some(previous) = previous_binding_ty {
                        self.value_types.insert(stmt.name.clone(), previous);
                    } else {
                        self.value_types.remove(&stmt.name);
                    }
                }
                format!("rsscript_runtime::{combinator}({pending}, |{bind}| {{ {rest} }})")
            }
            Stmt::Expr(expr) if async_await_inner(expr).is_some() => {
                let combinator = if async_await_is_try(expr) {
                    "pending_try"
                } else {
                    "pending_then"
                };
                let pending = self.lower_expr(async_await_inner(expr).expect("await inner"));
                let rest = self.lower_async_loop_body_chain(tail, error_ty);
                format!("rsscript_runtime::{combinator}({pending}, |_rsscript_unit| {{ {rest} }})")
            }
            other => {
                let mut statement_out = String::new();
                self.lower_stmt(other, &mut statement_out, 0);
                let rest = self.lower_async_loop_body_chain(tail, error_ty);
                format!("{{ {} {rest} }}", statement_out.trim())
            }
        }
    }

    pub(super) fn current_result_error_type_rust(&self) -> Option<String> {
        let ty = self.current_return_type.as_ref()?;
        if ty.name == "Result" && ty.args.len() == 2 {
            Some(self.lower_type_ref(&ty.args[1], ManagedPosition::Return))
        } else {
            None
        }
    }

    pub(super) fn lower_await_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Try { value, .. } => format!("{}?", self.lower_await_expr(value)),
            Expr::Effect { value, .. } => self.lower_await_expr(value),
            Expr::Call { callee, .. } if self.await_call_lowers_to_pending(callee) => {
                let pending = self.lower_expr(expr);
                self.current_async_executor
                    .as_ref()
                    .map(|executor| format!("{executor}.run_pending({pending})"))
                    .unwrap_or_else(|| format!("rsscript_runtime::run_pending({pending})"))
            }
            Expr::Call { .. } => self.lower_expr(expr),
            _ => self.lower_expr(expr),
        }
    }
}
