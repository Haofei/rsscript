use super::*;

impl Analyzer<'_> {
    pub(super) fn check_unsupported_syntax(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::declaration_surface_diagnostics(
                self.tokens,
                &self.syntax_program,
            ));
        let items = self.syntax_program.items.clone();
        for item in &items {
            self.check_unsupported_syntax_item(item);
        }
    }

    pub(super) fn check_unsupported_syntax_item(&mut self, item: &Item) {
        self.diagnostics
            .extend(rsscript_semantics::declaration_item_surface_diagnostics(
                item,
            ));
        self.diagnostics
            .extend(rsscript_semantics::item_body_surface_diagnostics(item));
        match item {
            Item::Function(function) => {
                for param in &function.params {
                    let canonical = self.canonical_type_ref(&param.ty);
                    if let Some(diagnostic) =
                        rsscript_semantics::by_value_callback_parameter_diagnostic(
                            &canonical,
                            param.effect,
                            &param.span,
                        )
                    {
                        self.diagnostics.push(diagnostic);
                    }
                    self.diagnostics
                        .extend(rsscript_semantics::type_ref_surface_diagnostics(
                            &canonical, true, true,
                        ));
                }
                if let Some(return_ty) = &function.return_ty {
                    // Return type is a storable position: `owned Fn(...)` may be
                    // returned (first-class), but `noescape` may not escape.
                    let canonical = self.canonical_type_ref(return_ty);
                    self.diagnostics
                        .extend(rsscript_semantics::type_ref_surface_diagnostics(
                            &canonical, false, true,
                        ));
                }
                self.check_canonical_type_refs_block(&function.body);
            }
            Item::Type(type_decl) => {
                self.diagnostics
                    .extend(rsscript_semantics::derive_syntax_diagnostics(
                        &type_decl.derives,
                        &type_decl.span,
                        type_decl.kind == TypeKind::Resource,
                    ));
                for field in &type_decl.fields {
                    // Struct/class fields are storable positions: an `owned Fn`
                    // field is first-class; `noescape` fields are rejected.
                    let canonical = self.canonical_type_ref(&field.ty);
                    self.diagnostics
                        .extend(rsscript_semantics::type_ref_surface_diagnostics(
                            &canonical, false, true,
                        ));
                }
            }
            Item::SumType(sum) => {
                self.diagnostics
                    .extend(rsscript_semantics::derive_syntax_diagnostics(
                        &sum.derives,
                        &sum.span,
                        false,
                    ));
                for field in sum.variants.iter().flat_map(|variant| &variant.fields) {
                    let canonical = self.canonical_type_ref(&field.ty);
                    self.diagnostics
                        .extend(rsscript_semantics::type_ref_surface_diagnostics(
                            &canonical, false, true,
                        ));
                }
            }
            Item::Const(_) => {}
            Item::Module(_) | Item::Use(_) | Item::TypeAlias(_) => {}
        }
    }

    /// Extract alias-canonical type-reference facts from bodies. All source
    /// syntax legality for these bodies is owned by `rsscript-semantics`.
    pub(super) fn check_canonical_type_refs_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.check_canonical_type_refs_stmt(statement);
        }
    }

    fn check_canonical_type_refs_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(ty) = &stmt.type_annotation {
                    let canonical = self.canonical_type_ref(ty);
                    self.diagnostics
                        .extend(rsscript_semantics::type_ref_surface_diagnostics(
                            &canonical, false, true,
                        ));
                }
                if let Some(value) = &stmt.value {
                    self.check_canonical_type_refs_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_canonical_type_refs_expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_canonical_type_refs_expr(&stmt.resource);
                self.check_canonical_type_refs_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_canonical_type_refs_expr(&stmt.condition);
                self.check_canonical_type_refs_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_canonical_type_refs_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_canonical_type_refs_expr(condition);
                }
                self.check_canonical_type_refs_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_canonical_type_refs_expr(&stmt.iterable);
                self.check_canonical_type_refs_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => self.check_canonical_type_refs_block(&stmt.body),
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_canonical_type_refs_expr(&arm.operation);
                    self.check_canonical_type_refs_block(&arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_canonical_type_refs_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_canonical_type_refs_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_canonical_type_refs_expr(&stmt.value);
                self.check_canonical_type_refs_block(&stmt.else_body);
            }
            Stmt::Assign(stmt) => {
                self.check_canonical_type_refs_expr(&stmt.target);
                self.check_canonical_type_refs_expr(&stmt.value);
            }
            Stmt::Expr(expr) => self.check_canonical_type_refs_expr(expr),
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

    fn check_canonical_type_refs_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.check_canonical_type_refs_expr(left);
                self.check_canonical_type_refs_expr(right);
            }
            Expr::Field { base, .. } => self.check_canonical_type_refs_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_canonical_type_refs_expr(base);
                self.check_canonical_type_refs_expr(index);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.check_canonical_type_refs_expr(&arg.value);
                }
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
                self.check_canonical_type_refs_expr(value);
            }
            Expr::Spawn { value, .. } | Expr::Await { value, .. } => {
                self.check_canonical_type_refs_expr(value);
            }
            Expr::Closure { body, .. } => self.check_canonical_type_refs_block(body),
            Expr::Match { value, arms, .. } => {
                self.check_canonical_type_refs_expr(value);
                for arm in arms {
                    self.check_canonical_type_refs_block(&arm.body);
                }
            }
            Expr::ObjectLiteral { fields, .. } => {
                for field in fields {
                    self.check_canonical_type_refs_expr(&field.value);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_canonical_type_refs_expr(&entry.key);
                    self.check_canonical_type_refs_expr(&entry.value);
                }
            }
            Expr::ArrayLiteral { items, .. } => {
                for item in items {
                    self.check_canonical_type_refs_expr(item);
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
}
