use super::*;

impl Analyzer<'_> {
    pub(crate) fn check_unknown_types(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_type_diagnostics(
                &self.hir,
                &self.syntax_program,
                &self.visible_protocol_names(),
            ));
    }

    pub(crate) fn check_unknown_fields(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_field_diagnostics(&self.hir));
    }

    pub(crate) fn check_unknown_bindings(&mut self) {
        // Collect top-level const names and sum type variant names as globally visible bindings
        let mut global_names: HashSet<String> = HashSet::new();
        for item in &self.syntax_program.items {
            match item {
                Item::Const(decl) => {
                    global_names.insert(decl.name.clone());
                }
                Item::SumType(sum) => {
                    for variant in &sum.variants {
                        global_names.insert(variant.name.clone());
                    }
                }
                _ => {}
            }
        }
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let Some(block) = self
                .hir
                .function_body(&function.name)
                .and_then(|body| body.block.clone())
            else {
                continue;
            };
            let mut visible: HashSet<String> = function
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect();
            visible.extend(global_names.iter().cloned());
            self.check_unknown_bindings_in_block(&block, &mut visible);
        }
    }

    pub(super) fn check_unknown_bindings_in_block(
        &mut self,
        block: &HirBlock,
        visible: &mut HashSet<String>,
    ) {
        for statement in &block.statements {
            self.check_unknown_bindings_in_stmt(statement, visible);
        }
    }

    pub(super) fn check_unknown_bindings_in_stmt(
        &mut self,
        statement: &HirStmt,
        visible: &mut HashSet<String>,
    ) {
        match statement {
            HirStmt::Let { name, value, .. } => {
                if let Some(value) = value {
                    self.check_unknown_bindings_in_expr(value, visible);
                }
                visible.insert(name.clone());
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.check_unknown_bindings_in_expr(value, visible);
                }
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(resource, visible);
                let mut body_visible = visible.clone();
                body_visible.insert(binding.clone());
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(condition, visible);
                let mut then_visible = visible.clone();
                self.check_unknown_bindings_in_block(then_body, &mut then_visible);
                if let Some(else_body) = else_body {
                    let mut else_visible = visible.clone();
                    self.check_unknown_bindings_in_block(else_body, &mut else_visible);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.check_unknown_bindings_in_expr(condition, visible);
                }
                let mut body_visible = visible.clone();
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(iterable, visible);
                let mut body_visible = visible.clone();
                body_visible.insert(binding.clone());
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::Match { value, arms, .. } => {
                self.check_unknown_bindings_in_expr(value, visible);
                for arm in arms {
                    let mut arm_visible = visible.clone();
                    for binding in arm.pattern.binding_names() {
                        arm_visible.insert(binding.to_string());
                    }
                    if let Some(guard) = &arm.guard {
                        self.check_unknown_bindings_in_expr(guard, &arm_visible);
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    self.check_unknown_bindings_in_expr(&arm.operation, visible);
                    let mut arm_visible = visible.clone();
                    if arm.binding != "_" {
                        arm_visible.insert(arm.binding.clone());
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirStmt::Expr(value) => self.check_unknown_bindings_in_expr(value, visible),
            HirStmt::Assign { target, value, .. } => {
                for read in crate::hir::assign_target_reads(target) {
                    self.check_unknown_bindings_in_expr(read, visible);
                }
                self.check_unknown_bindings_in_expr(value, visible);
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }

    pub(super) fn check_unknown_bindings_in_expr(
        &mut self,
        expr: &HirExpr,
        visible: &HashSet<String>,
    ) {
        match expr {
            HirExpr::Ident { name, span, .. } => {
                if !visible.contains(name) && !builtin_value_ident(name) {
                    self.unknown_binding_diagnostic(name, span);
                }
            }
            HirExpr::Binary { left, right, .. } => {
                self.check_unknown_bindings_in_expr(left, visible);
                self.check_unknown_bindings_in_expr(right, visible);
            }
            HirExpr::Field { base, .. } => self.check_unknown_bindings_in_expr(base, visible),
            HirExpr::Index { base, index, .. } => {
                self.check_unknown_bindings_in_expr(base, visible);
                self.check_unknown_bindings_in_expr(index, visible);
            }
            HirExpr::Call { args, .. } => {
                for arg in args {
                    self.check_unknown_bindings_in_expr(&arg.value, visible);
                }
            }
            HirExpr::Effect { value, .. }
            | HirExpr::Manage { value, .. }
            | HirExpr::Spawn { value, .. }
            | HirExpr::Await { value, .. }
            | HirExpr::Try { value, .. } => self.check_unknown_bindings_in_expr(value, visible),
            HirExpr::Closure { params, body, .. } => {
                let mut closure_visible = visible.clone();
                closure_visible.extend(params.iter().cloned());
                self.check_unknown_bindings_in_block(body, &mut closure_visible);
            }
            HirExpr::Match { value, arms, .. } => {
                self.check_unknown_bindings_in_expr(value, visible);
                for arm in arms {
                    let mut arm_visible = visible.clone();
                    for binding in arm.pattern.binding_names() {
                        arm_visible.insert(binding.to_string());
                    }
                    if let Some(guard) = &arm.guard {
                        self.check_unknown_bindings_in_expr(guard, &arm_visible);
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirExpr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_unknown_bindings_in_expr(&entry.key, visible);
                    self.check_unknown_bindings_in_expr(&entry.value, visible);
                }
            }
            HirExpr::ObjectLiteral { .. }
            | HirExpr::ArrayLiteral { .. }
            | HirExpr::Number { .. }
            | HirExpr::String { .. }
            | HirExpr::Char { .. }
            | HirExpr::Unknown(_) => {}
        }
    }

    pub(crate) fn check_fresh_generic_return_bound(
        &mut self,
        function_name: &str,
        return_ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        let target = fresh_return_target_type(return_ty);
        // A protocol method's implicit `Self` parameter is bound `Managed`, which
        // still admits a `fresh Self` return: managed structs/sums are freshly
        // ownable, and the per-instantiation derive (`derives(Clone)`) is checked
        // at the use site. A `fresh Self` from a value scalar is impossible
        // because scalars do not satisfy the protocol's `Managed` `Self` bound.
        let bound = bounds.get(&target.name).and_then(Option::as_ref);
        let fresh_bound_ok = matches!(bound, Some(GenericBound::Struct))
            || (target.name == "Self" && matches!(bound, Some(GenericBound::Managed)));
        if bounds.contains_key(&target.name) && !fresh_bound_ok {
            self.diagnostics.push(
                Diagnostic::error(
                    code::INVALID_FRESH_RETURN_TYPE,
                    format!(
                        "function `{function_name}` returns `fresh {}` but `{}` is not bounded by `Struct`.",
                        target.name, target.name
                    ),
                    target.span.clone(),
                    "invalid fresh generic type",
                )
                .with_cause("A generic `fresh T` return must require `T: Struct` so freshness is valid for every instantiation.")
                .with_fix(
                    "add_struct_bound",
                    format!("Declare `{}` with `{}: Struct`, or remove `fresh`.", target.name, target.name),
                    "manual",
                ),
            );
        }
    }

    pub(crate) fn check_resource_type_param_field(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        if bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource) {
            self.generic_resource_argument_diagnostic(
                &ty.name,
                &ty.name,
                &ty.span,
                "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
            );
        }
        for arg in &ty.args {
            self.check_resource_type_param_field(arg, bounds);
        }
    }
}
