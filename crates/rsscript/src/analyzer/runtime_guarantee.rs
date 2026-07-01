use super::*;

impl Analyzer<'_> {
    pub(super) fn check_try_operator_result_returns(&mut self) {
        for (index, item) in self.syntax_program.items.iter().enumerate() {
            let Item::Function(function) = item else {
                continue;
            };
            // `?` short-circuits on the failure variant of the function's return
            // type: `Err` for `Result`, `None` for `Option`. Both are permitted.
            if function
                .return_ty
                .as_ref()
                .is_some_and(|return_ty| matches!(return_ty.name.as_str(), "Result" | "Option"))
            {
                continue;
            }

            let start = self
                .tokens
                .iter()
                .position(|token| token.span == function.span)
                .unwrap_or(0);
            let end = self
                .syntax_program
                .items
                .iter()
                .skip(index + 1)
                .map(item_span)
                .find_map(|span| self.tokens.iter().position(|token| token.span == *span))
                .unwrap_or(self.tokens.len());

            for token in &self.tokens[start..end] {
                if token.symbol("?") {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_TRY_OPERATOR,
                            format!(
                                "`?` in `{}` requires the function to return `Result<T, E>`.",
                                function.name
                            ),
                            token.span.clone(),
                            "invalid try operator",
                        )
                        .with_cause("RSScript represents recoverable failure in explicit `Result` return types.")
                        .with_fix(
                            "return_result_or_handle_error",
                            "Change the return type to `Result<..., E>` or handle the error explicitly.",
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    pub(super) fn check_runtime_guarantee_bodies(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            for guarantee in RuntimeGuarantee::ALL {
                if function_has_effect(function, guarantee.effect_name()) {
                    self.check_runtime_guarantee_block(guarantee, &function.name, &function.body);
                }
            }
        }
    }

    pub(super) fn check_runtime_guarantee_block(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        block: &Block,
    ) {
        for statement in &block.statements {
            self.check_runtime_guarantee_stmt(guarantee, function_name, statement);
        }
    }

    pub(super) fn check_runtime_guarantee_stmt(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        statement: &Stmt,
    ) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_runtime_guarantee_expr(guarantee, function_name, value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_runtime_guarantee_expr(guarantee, function_name, value);
                }
            }
            Stmt::Assign(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.target);
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
            }
            Stmt::Expr(value) => self.check_runtime_guarantee_expr(guarantee, function_name, value),
            Stmt::With(stmt) => {
                if guarantee == RuntimeGuarantee::Pure {
                    self.pure_with_resource_diagnostic(function_name, &stmt.span);
                }
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.resource);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.condition);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_runtime_guarantee_block(guarantee, function_name, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_runtime_guarantee_expr(guarantee, function_name, condition);
                }
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.iterable);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &arm.operation);
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
                for arm in &stmt.arms {
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.else_body);
            }
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

    pub(super) fn check_runtime_guarantee_expr(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        expr: &Expr,
    ) {
        match expr {
            Expr::Call {
                callee, args, span, ..
            } => {
                match self.hir.resolve_call(callee) {
                    CallResolution::Resolved { signature, kind } => {
                        if matches!(kind, ResolvedCalleeKind::Constructor { .. }) {
                            if guarantee == RuntimeGuarantee::Noalloc {
                                self.noalloc_allocation_diagnostic(
                                    function_name,
                                    span,
                                    format!(
                                        "constructor `{}` creates a new value.",
                                        callee_display(callee)
                                    ),
                                );
                            }
                        } else if !signature
                            .effects
                            .iter()
                            .any(|effect| effect == guarantee.effect_name())
                        {
                            self.runtime_guarantee_call_diagnostic(
                                guarantee,
                                function_name,
                                callee,
                                span,
                            );
                        }
                    }
                    CallResolution::EnumVariant
                    | CallResolution::Ambiguous { .. }
                    | CallResolution::Unknown => {}
                }
                for arg in args {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &arg.value);
                }
            }
            Expr::Manage { value, span } => {
                if guarantee == RuntimeGuarantee::Noalloc {
                    self.noalloc_allocation_diagnostic(
                        function_name,
                        span,
                        "`manage` may allocate while migrating a local graph.".to_string(),
                    );
                }
                if guarantee == RuntimeGuarantee::Pure {
                    self.pure_manage_diagnostic(function_name, span);
                }
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
            }
            Expr::Effect { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
            }
            Expr::Binary { left, right, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, left);
                self.check_runtime_guarantee_expr(guarantee, function_name, right);
            }
            Expr::Field { base, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, base);
            }
            Expr::Index { base, index, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, base);
                self.check_runtime_guarantee_expr(guarantee, function_name, index);
            }
            Expr::Closure { body, .. } => {
                self.check_runtime_guarantee_block(guarantee, function_name, body);
            }
            Expr::Match { value, arms, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
                for arm in arms {
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &entry.key);
                    self.check_runtime_guarantee_expr(guarantee, function_name, &entry.value);
                }
            }
            Expr::ObjectLiteral { .. }
            | Expr::ArrayLiteral { .. }
            | Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::CharLiteral(_, _)
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }
}
