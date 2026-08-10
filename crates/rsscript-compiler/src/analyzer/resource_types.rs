use super::*;

impl Analyzer<'_> {
    pub(crate) fn check_resource_generic_arguments(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for field in &decl.fields {
                        self.check_resource_generic_type_ref(
                            &field.ty,
                            ResourceGenericContext::Ordinary,
                        );
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_generic_type_ref(
                            &param.ty,
                            ResourceGenericContext::Ordinary,
                        );
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_generic_type_ref(
                            return_ty,
                            ResourceGenericContext::Return,
                        );
                    }
                    self.check_resource_generic_calls_in_block(&function.body);
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    pub(super) fn check_resource_generic_type_ref(
        &mut self,
        ty: &TypeRef,
        context: ResourceGenericContext,
    ) {
        for (index, arg) in ty.args.iter().enumerate() {
            if self.hir.type_kind(&arg.name) == Some(HirTypeKind::Resource)
                && !resource_result_return_arg_allowed(ty, index, context)
            {
                self.resource_generic_argument_diagnostic(&ty.name, &arg.name, &arg.span);
            }
        }
        for (index, arg) in ty.args.iter().enumerate() {
            if resource_result_return_arg_allowed(ty, index, context) {
                continue;
            }
            self.check_resource_generic_type_ref(arg, ResourceGenericContext::Ordinary);
        }
    }

    pub(super) fn check_resource_generic_calls_in_block(
        &mut self,
        block: &crate::syntax::ast::Block,
    ) {
        for statement in &block.statements {
            self.check_resource_generic_calls_in_stmt(statement);
        }
    }

    pub(super) fn check_resource_generic_calls_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_generic_calls_in_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_generic_calls_in_expr(value);
                }
            }
            Stmt::Assign(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.target);
                self.check_resource_generic_calls_in_expr(&stmt.value);
            }
            Stmt::Expr(value) => self.check_resource_generic_calls_in_expr(value),
            Stmt::With(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.resource);
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.condition);
                self.check_resource_generic_calls_in_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_resource_generic_calls_in_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_resource_generic_calls_in_expr(condition);
                }
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.iterable);
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_resource_generic_calls_in_expr(&arm.operation);
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.value);
                self.check_resource_generic_calls_in_block(&stmt.else_body);
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

    pub(super) fn check_resource_generic_calls_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                if let Callee::Qualified { namespace, .. } = callee
                    && let Some((root, args)) = generic_namespace_args(namespace)
                {
                    for arg in args {
                        if self.hir.type_kind(arg) == Some(HirTypeKind::Resource) {
                            self.resource_generic_argument_diagnostic(root, arg, span);
                        }
                    }
                }
                for arg in args {
                    self.check_resource_generic_calls_in_expr(&arg.value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_resource_generic_calls_in_expr(left);
                self.check_resource_generic_calls_in_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_resource_generic_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_generic_calls_in_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_resource_generic_calls_in_expr(base);
                self.check_resource_generic_calls_in_expr(index);
            }
            Expr::Closure { body, .. } => self.check_resource_generic_calls_in_block(body),
            Expr::Match { value, arms, .. } => {
                self.check_resource_generic_calls_in_expr(value);
                for arm in arms {
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_resource_generic_calls_in_expr(&entry.key);
                    self.check_resource_generic_calls_in_expr(&entry.value);
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
