use super::*;

impl Analyzer<'_> {
    pub(super) fn check_unknown_types(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    let generic_params = decl
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<HashSet<_>>();
                    for field in &decl.fields {
                        self.check_unknown_type_ref(&field.ty, &generic_params);
                    }
                }
                Item::Function(function) => {
                    let generic_params = function
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<HashSet<_>>();
                    for param in &function.params {
                        self.check_unknown_type_ref(&param.ty, &generic_params);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_unknown_type_ref(return_ty, &generic_params);
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

    pub(super) fn check_unknown_type_ref(&mut self, ty: &TypeRef, generic_params: &HashSet<&str>) {
        if ty.name == "Capability" {
            self.check_capability_type_ref(ty, generic_params);
            for param in &ty.fn_params {
                self.check_unknown_type_ref(param, generic_params);
            }
            if let Some(return_ty) = &ty.fn_return {
                self.check_unknown_type_ref(return_ty, generic_params);
            }
            return;
        }
        // Type aliases are known types
        if self.type_aliases.contains_key(&ty.name) {
            // Valid - it's a type alias
        } else if !known_type_ref(ty, generic_params, &self.hir) {
            self.unknown_type_diagnostic(ty);
        }
        for arg in &ty.args {
            self.check_unknown_type_ref(arg, generic_params);
        }
        for param in &ty.fn_params {
            self.check_unknown_type_ref(param, generic_params);
        }
        if let Some(return_ty) = &ty.fn_return {
            self.check_unknown_type_ref(return_ty, generic_params);
        }
    }

    pub(super) fn check_capability_type_ref(
        &mut self,
        ty: &TypeRef,
        generic_params: &HashSet<&str>,
    ) {
        if ty.args.len() != 1 {
            self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
            return;
        }
        let protocol = &ty.args[0];
        if !protocol.args.is_empty()
            || !protocol.fn_params.is_empty()
            || protocol.fn_return.is_some()
            || protocol.is_fresh
            || protocol.is_noescape
            || protocol.is_owned
        {
            self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
            return;
        }
        if generic_params.contains(protocol.name.as_str()) {
            return;
        }
        if !self.protocol_name_is_visible(&protocol.name) {
            self.unknown_protocol_diagnostic(&protocol.name, &protocol.span);
        }
    }

    pub(super) fn check_unknown_fields(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let Some(body) = self.hir.function_body(&function.name).cloned() else {
                continue;
            };
            for access in &body.field_accesses {
                let Some(base_type) = &access.base_type else {
                    continue;
                };
                let Some(type_info) = self.hir.type_info(base_type) else {
                    continue;
                };
                if !type_info.fields.contains_key(&access.name) {
                    self.unknown_field_diagnostic(&access.name, base_type, &access.span);
                }
            }
        }
    }

    pub(super) fn check_unknown_bindings(&mut self) {
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
            | HirExpr::Unknown(_) => {}
        }
    }

    pub(super) fn check_fresh_generic_return_bound(
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

    pub(super) fn check_generic_resource_pool_type_ref(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        if ty.name == "ResourcePool"
            && let Some(arg) = ty.args.first()
            && let Some(bound) = bounds.get(&arg.name)
            && bound.as_ref() != Some(&GenericBound::Resource)
        {
            self.invalid_resource_pool_type_diagnostic(
                format!(
                    "ResourcePool<{}> requires `{}` to be explicitly bounded by Resource.",
                    arg.name, arg.name
                ),
                arg.span.clone(),
            );
        }
        for arg in &ty.args {
            self.check_generic_resource_pool_type_ref(arg, bounds);
        }
    }

    pub(super) fn check_resource_type_param_field(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
        in_resource_pool: bool,
    ) {
        let next_in_resource_pool =
            in_resource_pool || self.is_approved_resource_container(&ty.name);
        if !next_in_resource_pool
            && bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource)
        {
            self.generic_resource_argument_diagnostic(
                &ty.name,
                &ty.name,
                &ty.span,
                "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
            );
        }
        for arg in &ty.args {
            self.check_resource_type_param_field(arg, bounds, next_in_resource_pool);
        }
    }
}
