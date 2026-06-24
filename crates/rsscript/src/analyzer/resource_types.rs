use super::*;

impl Analyzer<'_> {
    /// A type allowed to hold `T: Resource` directly. For now this is only the
    /// built-in `ResourcePool`: it owns the lease/return/discard machinery that
    /// keeps resource access scoped. A user-facing extension is deferred — a plain
    /// marker `impl` would be a self-service whitelist that bypasses `RS0701` with
    /// none of those guarantees, so any extension must be a compiler-recognized
    /// declaration with explicit approval conditions, not an open `impl`.
    pub(super) fn is_approved_resource_container(&self, type_name: &str) -> bool {
        type_name == "ResourcePool"
    }

    pub(super) fn check_resource_fields(&mut self) {
        for item in &self.syntax_program.items {
            let Item::Type(decl) = item else {
                continue;
            };
            if self.hir.type_kind(&decl.name) == Some(HirTypeKind::Resource) {
                continue;
            }
            for field in &decl.fields {
                if self.hir.type_kind(&field.ty.name) == Some(HirTypeKind::Resource) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::RESOURCE_FIELD,
                            format!("resource `{}` cannot be stored in `{}`.", field.ty.name, decl.name),
                            field.span.clone(),
                            "resource field",
                        )
                        .with_cause("Resources must be used through `with` or approved resource containers.")
                        .with_fix("use_with", "Use `with` or `ResourcePool<T: Resource>` instead.", "manual"),
                    );
                }
            }
        }
    }

    pub(super) fn check_fd_surface(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Function(function) => {
                    if function.is_native {
                        continue;
                    }
                    for param in &function.params {
                        if type_ref_contains_name(&param.ty, "Fd") {
                            self.fd_surface_diagnostic(
                                param.ty.span.clone(),
                                "`Fd` parameter outside native boundary",
                                "Use a `resource` type such as `File` instead of exposing raw descriptor handles.",
                            );
                        }
                    }
                    if let Some(return_ty) = &function.return_ty
                        && type_ref_contains_name(return_ty, "Fd")
                    {
                        self.fd_surface_diagnostic(
                            return_ty.span.clone(),
                            "`Fd` return outside native boundary",
                            "Return a `resource` type such as `File` instead of exposing raw descriptor handles.",
                        );
                    }
                }
                Item::Type(decl) => {
                    if decl.kind == TypeKind::Resource {
                        continue;
                    }
                    for field in &decl.fields {
                        if type_ref_contains_name(&field.ty, "Fd") {
                            self.fd_surface_diagnostic(
                                field.ty.span.clone(),
                                "`Fd` field outside resource internals",
                                "Use a `resource` field wrapper or a non-Fd public value type.",
                            );
                        }
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    pub(super) fn check_weak_fields(&mut self) {
        for item in &self.syntax_program.items {
            let Item::Type(decl) = item else {
                continue;
            };
            for field in &decl.fields {
                if !field.is_weak {
                    continue;
                }
                if self.hir.type_kind(&field.ty.name) != Some(HirTypeKind::Class) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_WEAK_FIELD,
                            format!(
                                "weak field `{}` must point to a class, but `{}` is not a class.",
                                field.name, field.ty.name
                            ),
                            field.span.clone(),
                            "invalid weak field",
                        )
                        .with_cause(
                            "`weak` is only for breaking managed identity-object cycles in the MVP.",
                        )
                        .with_fix(
                            "use_class_or_remove_weak",
                            "Use a class type for the weak field, or remove `weak`.",
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    pub(super) fn check_resource_pool_type_arguments(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for field in &decl.fields {
                        self.check_resource_pool_type_ref(&field.ty);
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_pool_type_ref(&param.ty);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_pool_type_ref(return_ty);
                    }
                    self.check_resource_pool_calls_in_block(&function.body);
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    pub(super) fn check_resource_generic_arguments(&mut self) {
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

    pub(super) fn check_resource_pool_type_ref(&mut self, ty: &TypeRef) {
        if ty.name == "ResourcePool" {
            match ty.args.first() {
                Some(arg) => self.check_resource_pool_arg(&arg.name, &arg.span),
                None => self.invalid_resource_pool_type_diagnostic(
                    "ResourcePool must declare a resource type argument.",
                    ty.span.clone(),
                ),
            }
        }
        for arg in &ty.args {
            self.check_resource_pool_type_ref(arg);
        }
    }

    pub(super) fn check_resource_pool_calls_in_block(&mut self, block: &crate::syntax::ast::Block) {
        for statement in &block.statements {
            self.check_resource_pool_calls_in_stmt(statement);
        }
    }

    pub(super) fn check_resource_generic_type_ref(&mut self, ty: &TypeRef, context: ResourceGenericContext) {
        if ty.name != "ResourcePool" {
            for (index, arg) in ty.args.iter().enumerate() {
                if self.hir.type_kind(&arg.name) == Some(HirTypeKind::Resource)
                    && !resource_result_return_arg_allowed(ty, index, context)
                {
                    self.resource_generic_argument_diagnostic(&ty.name, &arg.name, &arg.span);
                }
            }
        }
        for (index, arg) in ty.args.iter().enumerate() {
            if resource_result_return_arg_allowed(ty, index, context) {
                continue;
            }
            self.check_resource_generic_type_ref(arg, ResourceGenericContext::Ordinary);
        }
    }

    pub(super) fn check_resource_pool_calls_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_pool_calls_in_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_pool_calls_in_expr(value);
                }
            }
            Stmt::Assign(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.target);
                self.check_resource_pool_calls_in_expr(&stmt.value);
            }
            Stmt::Expr(value) => self.check_resource_pool_calls_in_expr(value),
            Stmt::With(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.resource);
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.condition);
                self.check_resource_pool_calls_in_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_resource_pool_calls_in_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_resource_pool_calls_in_expr(condition);
                }
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.iterable);
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_resource_pool_calls_in_expr(&arm.operation);
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.value);
                self.check_resource_pool_calls_in_block(&stmt.else_body);
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

    pub(super) fn check_resource_pool_calls_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                if let Callee::Qualified { namespace, name } = callee
                    && namespace == "ResourcePool"
                    && name == "new"
                {
                    self.invalid_resource_pool_type_diagnostic(
                        "ResourcePool.new must be called as ResourcePool<T>.new with resource T.",
                        span.clone(),
                    );
                } else if let Callee::Qualified { namespace, .. } = callee
                    && let Some(arg) = resource_pool_namespace_arg(namespace)
                {
                    self.check_resource_pool_arg(arg, span);
                }
                for arg in args {
                    self.check_resource_pool_calls_in_expr(&arg.value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_resource_pool_calls_in_expr(left);
                self.check_resource_pool_calls_in_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_resource_pool_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_pool_calls_in_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_resource_pool_calls_in_expr(base);
                self.check_resource_pool_calls_in_expr(index);
            }
            Expr::Closure { body, .. } => self.check_resource_pool_calls_in_block(body),
            Expr::Match { value, arms, .. } => {
                self.check_resource_pool_calls_in_expr(value);
                for arm in arms {
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_resource_pool_calls_in_expr(&entry.key);
                    self.check_resource_pool_calls_in_expr(&entry.value);
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

    pub(super) fn check_resource_generic_calls_in_block(&mut self, block: &crate::syntax::ast::Block) {
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
                    && !self.is_approved_resource_container(root)
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
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    pub(super) fn check_resource_pool_arg(&mut self, type_name: &str, span: &crate::diagnostic::Span) {
        match self.hir.type_kind(type_name) {
            Some(HirTypeKind::Resource) | None => {}
            Some(HirTypeKind::Class) | Some(HirTypeKind::Struct) | Some(HirTypeKind::Sum) => {
                self.invalid_resource_pool_type_diagnostic(
                    format!(
                        "ResourcePool can only hold resources, but `{type_name}` is not a resource."
                    ),
                    span.clone(),
                );
            }
        }
    }

}
