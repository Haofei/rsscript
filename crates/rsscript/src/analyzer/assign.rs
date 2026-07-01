use super::*;

#[derive(Debug, Clone, Copy)]
enum AssignBinding {
    MutLocal,
    ImmutableLocal,
    Param,
    /// A `mut` parameter: not reassignable itself, but its fields/elements may be
    /// updated in place (the mutation propagates to the caller, matching `&mut`).
    MutParam,
}

#[derive(Clone)]
struct AssignScopeEntry {
    binding: AssignBinding,
    type_name: Option<String>,
}

/// Validates controlled assignments: place/mutability rules plus a type check
/// (the value's type must match the place's type when both are known), so an
/// `Int = String` style error is reported in RSScript instead of leaking from
/// rustc. Binding kinds and their types are stored per lexical scope so an
/// inner shadow does not pollute the type of an outer same-named binding.
pub(super) struct AssignChecker<'a> {
    hir: &'a Hir,
    scopes: Vec<HashMap<String, AssignScopeEntry>>,
    pub(super) diagnostics: Vec<Diagnostic>,
}

impl<'a> AssignChecker<'a> {
    pub(super) fn new(hir: &'a Hir) -> Self {
        Self {
            hir,
            scopes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn check_function(&mut self, function: &FunctionDecl) {
        self.scopes.clear();
        self.push_scope();
        for param in &function.params {
            let binding = if param.effect == Some(DataEffect::Mut) {
                AssignBinding::MutParam
            } else {
                AssignBinding::Param
            };
            self.insert(param.name.clone(), binding, Some(type_ref_name(&param.ty)));
        }
        self.block(&function.body);
        self.pop_scope();
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn insert(&mut self, name: String, binding: AssignBinding, type_name: Option<String>) {
        if name.is_empty() {
            return;
        }
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, AssignScopeEntry { binding, type_name });
        }
    }

    fn resolve(&self, name: &str) -> Option<AssignBinding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).map(|entry| entry.binding))
    }

    /// The type of the innermost binding named `name` (its own type, even when
    /// that is unknown), so a shadowing binding does not expose an outer type.
    fn resolve_type(&self, name: &str) -> Option<String> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .and_then(|entry| entry.type_name.clone())
    }

    /// A flattened view of every currently-visible binding's type, with inner
    /// scopes shadowing outer ones — including an untyped inner binding hiding
    /// an outer type — for inferring the type of a value expression.
    fn current_value_types(&self) -> HashMap<String, String> {
        let mut value_types = HashMap::new();
        for scope in &self.scopes {
            for (name, entry) in scope {
                match &entry.type_name {
                    Some(type_name) => {
                        value_types.insert(name.clone(), type_name.clone());
                    }
                    None => {
                        value_types.remove(name);
                    }
                }
            }
        }
        value_types
    }

    fn block(&mut self, block: &Block) {
        self.push_scope();
        for statement in &block.statements {
            self.stmt(statement);
        }
        self.pop_scope();
    }

    fn stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Assign(stmt) => {
                self.validate_assignment(stmt);
                self.expr(&stmt.value);
            }
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.expr(value);
                }
                let type_name = stmt
                    .type_annotation
                    .as_ref()
                    .map(type_ref_name)
                    .or_else(|| stmt.value.as_ref().and_then(|value| self.infer_type(value)));
                self.insert(
                    stmt.name.clone(),
                    if stmt.is_mut {
                        AssignBinding::MutLocal
                    } else {
                        AssignBinding::ImmutableLocal
                    },
                    type_name,
                );
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.expr(&stmt.resource);
                self.push_scope();
                self.insert(stmt.binding.clone(), AssignBinding::ImmutableLocal, None);
                self.block(&stmt.body);
                self.pop_scope();
            }
            Stmt::If(stmt) => {
                self.expr(&stmt.condition);
                self.block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.expr(condition);
                }
                self.block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.expr(&stmt.iterable);
                self.push_scope();
                self.insert(stmt.binding.clone(), AssignBinding::ImmutableLocal, None);
                self.block(&stmt.body);
                self.pop_scope();
            }
            Stmt::Match(stmt) => {
                self.expr(&stmt.value);
                self.match_arms(&stmt.arms);
            }
            Stmt::TaskGroup(stmt) => self.block(&stmt.body),
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.expr(&arm.operation);
                    self.block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.expr(&stmt.value);
                self.block(&stmt.else_body);
                self.insert(
                    stmt.binding_name.clone(),
                    AssignBinding::ImmutableLocal,
                    None,
                );
            }
            Stmt::Expr(expr) => self.expr(expr),
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

    /// Walk into expressions that carry nested blocks (closures and match arms)
    /// so assignments inside them are validated against the right scope.
    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Closure { params, body, .. } => {
                self.closure(params, body, None);
            }
            Expr::Match { value, arms, .. } => {
                self.expr(value);
                self.match_arms(arms);
            }
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Field { base, .. } => self.expr(base),
            Expr::Index { base, index, .. } => {
                self.expr(base);
                self.expr(index);
            }
            Expr::Call { callee, args, .. } => {
                for (index, arg) in args.iter().enumerate() {
                    // A closure passed where an `owned Fn(.., mut T, ..)` is
                    // expected (struct/sum field or function parameter) binds
                    // that closure parameter as a `mut` parameter, so the body
                    // may update its fields/elements — mirroring how a `mut`
                    // function parameter behaves. The effects come from the
                    // expected Fn type's declared parameter effects.
                    if let Expr::Closure { params, body, .. } = &arg.value {
                        let expected_fn =
                            self.expected_fn_type_for_call_arg(callee, arg.name.as_deref(), index);
                        self.closure(params, body, expected_fn.as_deref());
                    } else {
                        self.expr(&arg.value);
                    }
                }
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => self.expr(value),
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.expr(&entry.key);
                    self.expr(&entry.value);
                }
            }
            Expr::ObjectLiteral { .. }
            | Expr::ArrayLiteral { .. }
            | Expr::Ident(..)
            | Expr::Number(..)
            | Expr::String(..)
            | Expr::CharLiteral(..)
            | Expr::MultilineString(..)
            | Expr::Unknown(_) => {}
        }
    }

    /// Validate a closure body in its own scope. Each closure parameter is an
    /// immutable local by default; when the closure fills a slot typed
    /// `... Fn(.., mut T, ..) -> ..`, the corresponding parameter is bound as a
    /// `mut` parameter so the body may update its fields/elements (e.g. a stored
    /// rule's `mut Ctx` parameter). The parameter binding itself stays
    /// non-rebindable, exactly like a `mut` function parameter.
    fn closure(&mut self, params: &[String], body: &Block, expected_fn_type: Option<&str>) {
        let param_effects = expected_fn_type
            .map(fn_type_param_effects)
            .unwrap_or_default();
        self.push_scope();
        for (index, param) in params.iter().enumerate() {
            let binding = if matches!(param_effects.get(index), Some(Some(DataEffect::Mut))) {
                AssignBinding::MutParam
            } else {
                AssignBinding::ImmutableLocal
            };
            let type_name = param_effects
                .get(index)
                .and_then(|_| fn_type_param_type_name(expected_fn_type?, index));
            self.insert(param.clone(), binding, type_name);
        }
        self.block(body);
        self.pop_scope();
    }

    /// The declared type of a call argument's target parameter/field, when the
    /// callee is a known struct/sum constructor or a resolved function. Used to
    /// thread the expected `Fn` type (and its parameter effects) into a closure
    /// argument's scope.
    fn expected_fn_type_for_call_arg(
        &self,
        callee: &Callee,
        arg_name: Option<&str>,
        index: usize,
    ) -> Option<String> {
        // Constructor field types (`RwRule(fxn: <closure>)`).
        if let Callee::Name(name) = callee {
            let root = type_root_name(name);
            if let Some(type_info) = self.hir.type_info(root) {
                let field = match arg_name {
                    Some(field_name) => type_info.fields.get(field_name),
                    None => type_info.fields_ordered.get(index),
                };
                if let Some(field) = field {
                    return Some(field.type_name.clone());
                }
            }
            if let Some(fields) = self.hir.sum_variant_fields(root) {
                let field = match arg_name {
                    Some(field_name) => fields.iter().find(|f| f.name == field_name),
                    None => fields.get(index),
                };
                if let Some(field) = field {
                    return Some(field.type_name.clone());
                }
            }
        }
        // Resolved function/method parameter types.
        if let CallResolution::Resolved { signature, .. } = self.hir.resolve_call(callee) {
            let param = match arg_name {
                Some(param_name) => signature.params.iter().find(|p| p.name == param_name),
                None => signature.params.get(index),
            };
            if let Some(param) = param {
                return Some(param.type_name.clone());
            }
        }
        None
    }

    fn match_arms(&mut self, arms: &[crate::syntax::ast::MatchArm]) {
        for arm in arms {
            self.push_scope();
            for binding in arm.pattern.binding_names() {
                self.insert(binding.to_string(), AssignBinding::ImmutableLocal, None);
            }
            if let Some(guard) = &arm.guard {
                self.expr(guard);
            }
            self.block(&arm.body);
            self.pop_scope();
        }
    }

    fn infer_type(&self, value: &Expr) -> Option<String> {
        crate::hir::infer_hir_expr_type(self.hir, value, &self.current_value_types())
    }

    fn validate_assignment(&mut self, stmt: &AssignStmt) {
        let span = stmt.target.span().clone();
        match &stmt.target {
            Expr::Ident(name, _) => {
                let (label, cause) = match self.resolve(name) {
                    Some(AssignBinding::MutLocal) => {
                        self.check_assignment_type(name, &stmt.value, &span);
                        return;
                    }
                    // A `mut` parameter of a Copy scalar type (Int/Bool/Float/Char, …)
                    // MAY be reassigned: it lowers to `&mut T`, and the new value is
                    // written back to the caller, matching `&mut` semantics. Non-Copy
                    // `mut` params (struct/collection) stay non-rebindable below.
                    Some(AssignBinding::MutParam)
                        if self
                            .resolve_type(name)
                            .is_some_and(|ty| crate::checks::local::is_copy_type_name(&ty)) =>
                    {
                        self.check_assignment_type(name, &stmt.value, &span);
                        return;
                    }
                    Some(AssignBinding::ImmutableLocal) => (
                        format!("`{name}` is an immutable binding"),
                        format!("Declare `{name}` with `let mut` to allow reassignment."),
                    ),
                    Some(AssignBinding::Param | AssignBinding::MutParam) => (
                        format!("`{name}` is a parameter, not a reassignable local"),
                        "Parameters are not reassignable (except a `mut` Copy-scalar one, which is written back to the caller): a non-Copy `mut` parameter's fields/elements may be updated, but the parameter binding itself can't be rebound. Bind a `let mut` local instead."
                            .to_string(),
                    ),
                    None => (
                        format!("`{name}` is not a binding in scope"),
                        "The left side of `=` must be a `let mut` local in scope.".to_string(),
                    ),
                };
                self.diagnostics
                    .push(invalid_assignment_diagnostic(span, label, cause));
            }
            Expr::Field { .. } | Expr::Index { .. } => {
                self.validate_compound_assignment(stmt, &span);
            }
            _ => self.diagnostics.push(invalid_assignment_diagnostic(
                span,
                "the left side of `=` is not a place".to_string(),
                "Assignment targets must be a place: a local, a field, or an index — not a call result or literal."
                    .to_string(),
            )),
        }
    }

    /// When both the place's type and the value's type are known, they must
    /// match. Unknown value types (such as the result of an unchecked binary
    /// expression) are left to the existing checks.
    fn check_assignment_type(&mut self, name: &str, value: &Expr, span: &crate::diagnostic::Span) {
        let Some(target_type) = self.resolve_type(name) else {
            return;
        };
        let Some(value_type) = self.infer_type(value) else {
            return;
        };
        if crate::checks::calls::unresolved_generic_type(&target_type)
            || crate::checks::calls::unresolved_generic_type(&value_type)
        {
            return;
        }
        if !crate::checks::calls::argument_type_matches(&target_type, &value_type) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::ASSIGNMENT_TYPE_MISMATCH,
                    format!("cannot assign `{value_type}` to `{name}` of type `{target_type}`."),
                    span.clone(),
                    "assignment type mismatch",
                )
                .with_cause(
                    "The assigned value's type must match the place's type before Rust lowering.",
                )
                .with_fix(
                    "match_assignment_type",
                    format!("Assign a `{target_type}` value to `{name}`."),
                    "manual",
                ),
            );
        }
    }

    fn validate_compound_assignment(&mut self, stmt: &AssignStmt, span: &crate::diagnostic::Span) {
        let Some(root) = assign_place_root(&stmt.target) else {
            self.diagnostics.push(invalid_assignment_diagnostic(
                span.clone(),
                "the left side of `=` is not rooted in a local place".to_string(),
                "Field and index assignment must start from a local binding, such as `user.name` or `items[i]`."
                    .to_string(),
            ));
            return;
        };
        match self.resolve(root) {
            Some(AssignBinding::MutLocal) => {}
            // A `mut` parameter's fields/elements may be updated in place (the
            // mutation propagates to the caller, like `&mut`).
            Some(AssignBinding::MutParam) => {}
            Some(AssignBinding::ImmutableLocal) => {
                self.diagnostics.push(invalid_assignment_diagnostic(
                    span.clone(),
                    format!("`{root}` is an immutable binding"),
                    format!("Declare `{root}` with `let mut` before assigning through one of its fields or indexes."),
                ));
                return;
            }
            Some(AssignBinding::Param) => {
                self.diagnostics.push(invalid_assignment_diagnostic(
                    span.clone(),
                    format!("`{root}` is a parameter, not a reassignable local"),
                    "Assign through a `mut` API parameter explicitly, or bind the value to a `let mut` local first."
                        .to_string(),
                ));
                return;
            }
            None => {
                self.diagnostics.push(invalid_assignment_diagnostic(
                    span.clone(),
                    format!("`{root}` is not a binding in scope"),
                    "The assignment target's root must be a `let mut` local in scope.".to_string(),
                ));
                return;
            }
        }

        if matches!(stmt.target, Expr::Index { .. }) {
            let Some(index_base) = assign_index_base(&stmt.target) else {
                return;
            };
            let Some(base_type) = self.infer_type(index_base) else {
                return;
            };
            if type_root_name(&base_type) != "List" {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::ASSIGNMENT_TARGET_DEFERRED,
                        "index assignment is only supported for List values.",
                        span.clone(),
                        format!("cannot assign through `{base_type}` index"),
                    )
                    .with_cause(
                        "`list[i] = value` has clear in-place list update semantics. Other indexed types still require explicit APIs such as `Map.insert`.",
                    )
                    .with_fix(
                        "use_explicit_update_api",
                        "Use the collection's explicit mutating API for this indexed assignment.",
                        "manual",
                    ),
                );
                return;
            }
        }

        self.check_assignment_place_type(&stmt.target, &stmt.value, span);
    }

    fn check_assignment_place_type(
        &mut self,
        target: &Expr,
        value: &Expr,
        span: &crate::diagnostic::Span,
    ) {
        let Some(target_type) = self.infer_type(target) else {
            return;
        };
        let Some(value_type) = self.infer_type(value) else {
            return;
        };
        if crate::checks::calls::unresolved_generic_type(&target_type)
            || crate::checks::calls::unresolved_generic_type(&value_type)
        {
            return;
        }
        if !crate::checks::calls::argument_type_matches(&target_type, &value_type) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::ASSIGNMENT_TYPE_MISMATCH,
                    format!("cannot assign `{value_type}` to `{target_type}` place."),
                    span.clone(),
                    "assignment type mismatch",
                )
                .with_cause(
                    "The assigned value's type must match the field or indexed element type before Rust lowering.",
                )
                .with_fix(
                    "match_assignment_type",
                    format!("Assign a `{target_type}` value to this place."),
                    "manual",
                ),
            );
        }
    }
}

fn invalid_assignment_diagnostic(
    span: crate::diagnostic::Span,
    label: String,
    cause: String,
) -> Diagnostic {
    Diagnostic::error(code::INVALID_ASSIGNMENT, "invalid assignment.", span, label)
        .with_cause(cause)
        .with_fix(
            "declare_let_mut",
            "Declare the target as a `let mut` local, or remove the assignment.",
            "manual",
        )
}

/// The root place identifier of an assignment target, following field/index
/// bases. Returns `None` when the target bottoms out at a non-place such as a
/// call result (`get_user().name = ...`).
fn assign_place_root(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => assign_place_root(base),
        _ => None,
    }
}

fn assign_index_base(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Index { base, .. } => Some(base),
        Expr::Field { base, .. } => assign_index_base(base),
        _ => None,
    }
}
