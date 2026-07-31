use super::*;

impl Analyzer<'_> {
    pub(super) fn check_unsupported_syntax(&mut self) {
        for span in self.syntax_program.unknown_top_level_spans.clone() {
            self.unsupported_syntax(
                span,
                "unsupported top-level item",
                "This top-level construct is outside the current RSScript parser surface.",
            );
        }
        for span in self.syntax_program.malformed_declaration_spans.clone() {
            self.unsupported_syntax(
                span,
                "malformed declaration",
                "This declaration starts like RSScript syntax but does not match the supported declaration grammar.",
            );
        }
        self.check_module_use_layout();
        self.check_reserved_declaration_names();
        self.check_reserved_protocol_generics();
        let items = self.syntax_program.items.clone();
        for item in &items {
            self.check_unsupported_syntax_item(item);
        }
    }

    fn check_reserved_protocol_generics(&mut self) {
        for index in 0..self.tokens.len().saturating_sub(2) {
            if !(self.tokens[index].is_ident_text("protocol")
                || self.tokens[index].is_ident_text("impl"))
                || !self.tokens[index + 2].symbol("<")
            {
                continue;
            }
            self.unsupported_syntax(
                self.tokens[index + 2].span.clone(),
                "generic protocol declaration",
                "Generic protocol and protocol-implementation declarations are reserved for a later language version; use function generics with a protocol bound instead.",
            );
        }
    }

    /// The `__rss_*` and `__rsscript_*` namespaces are reserved for
    /// compiler-generated desugaring temporaries and runtime helpers. Reject user
    /// declarations that claim one so generated helpers can never collide with
    /// source symbols. Other `__`-prefixed names (Python-style dunders like
    /// `__hash__`, and the synthetic `__TupleN` tuple structs) are left legal.
    pub(super) fn check_reserved_declaration_names(&mut self) {
        use crate::syntax::ast::Item;
        for item in self.syntax_program.items.clone() {
            let (name, span) = match &item {
                Item::Function(decl) => (decl.name.as_str(), &decl.span),
                Item::Type(decl) => (decl.name.as_str(), &decl.span),
                Item::SumType(decl) => (decl.name.as_str(), &decl.span),
                Item::TypeAlias(decl) => (decl.name.as_str(), &decl.span),
                Item::Const(decl) => (decl.name.as_str(), &decl.span),
                Item::Module(_) | Item::Use(_) => continue,
            };
            // `Type.method` reserves on the member, not the (user) type prefix.
            let leaf = name.rsplit('.').next().unwrap_or(name);
            if is_reserved_generated_name(leaf) {
                self.unsupported_syntax(
                    span.clone(),
                    "reserved declaration name",
                    "The `__rss_` and `__rsscript_` prefixes are reserved for compiler-generated symbols; rename this declaration.",
                );
            }
        }
    }

    pub(super) fn check_module_use_layout(&mut self) {
        // Layout is per source file: a merged multi-file program legitimately has
        // one `module` declaration per file, so the "at most one module" /
        // ordering rules are tracked by the declaration's originating file.
        let mut seen_module: HashSet<String> = HashSet::new();
        let mut seen_use: HashSet<String> = HashSet::new();
        let mut seen_non_organization_item: HashSet<String> = HashSet::new();
        // Per file, the local import names already bound, so a second import that
        // would silently shadow the first is rejected instead of overwritten.
        let mut seen_import_local: HashMap<String, HashSet<String>> = HashMap::new();
        let items = self.syntax_program.items.clone();
        for item in &items {
            let file = item_span_file(item);
            match item {
                Item::Module(module) => {
                    if seen_module.contains(&file) {
                        self.unsupported_syntax(
                            module.span.clone(),
                            "duplicate module declaration",
                            "A source or interface file may declare at most one `module` identity.",
                        );
                    }
                    if seen_non_organization_item.contains(&file) {
                        self.unsupported_syntax(
                            module.span.clone(),
                            "misplaced module declaration",
                            "`module` is source-organization metadata and must appear before declarations.",
                        );
                    }
                    if seen_use.contains(&file) {
                        self.unsupported_syntax(
                            module.span.clone(),
                            "misplaced module declaration",
                            "`module` must be the first organization declaration when present; `use` declarations follow it.",
                        );
                    }
                    seen_module.insert(file);
                }
                Item::Use(use_decl) => {
                    if seen_non_organization_item.contains(&file) {
                        self.unsupported_syntax(
                            use_decl.span.clone(),
                            "misplaced use declaration",
                            "`use` is source-organization metadata and must appear before declarations.",
                        );
                    }
                    if let Some(local) = use_decl.local_name()
                        && !seen_import_local
                            .entry(file.clone())
                            .or_default()
                            .insert(local.to_string())
                    {
                        self.unsupported_syntax(
                            use_decl.span.clone(),
                            "duplicate import name",
                            "Two `use` declarations bind the same local name in this file. Rename one with `use module.name as other_name` so each import is unambiguous.",
                        );
                    }
                    seen_use.insert(file);
                }
                Item::Type(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_)
                | Item::Function(_) => {
                    seen_non_organization_item.insert(file);
                }
            }
        }
    }

    pub(super) fn check_unsupported_syntax_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => {
                for span in &function.malformed_generic_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed generic parameter declaration",
                        "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                    );
                }
                if function.is_native && !function.body.statements.is_empty() {
                    self.unsupported_syntax(
                        function.span.clone(),
                        "unsupported native function body",
                        "`native fn` declares an external/native boundary in v0.7. Provide a bodyless declaration and bind the implementation through the native wrapper path.",
                    );
                }
                for span in &function.malformed_effect_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed effect declaration",
                        "Effects must use a bare effect name or `retains(parameter)`.",
                    );
                }
                for span in &function.malformed_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed parameter declaration",
                        "Function parameters must use `name: Type`, `name: read Type`, `name: mut Type`, or `name: take Type`.",
                    );
                }
                for param in &function.params {
                    let canonical = self.canonical_type_ref(&param.ty);
                    if param.effect == Some(DataEffect::Take)
                        && canonical.name == "Fn"
                        && !canonical.is_owned
                    {
                        self.unsupported_syntax(
                            param.span.clone(),
                            "unsupported by-value callback parameter",
                            "A callback passed with `take` must use `owned Fn(...)` so the Rust representation is sized. Use `read Fn(...)`, `mut Fn(...)`, or `take owned Fn(...)`.",
                        );
                    }
                    self.check_unsupported_syntax_type_ref(&canonical, true, true);
                }
                if let Some(return_ty) = &function.return_ty {
                    // Return type is a storable position: `owned Fn(...)` may be
                    // returned (first-class), but `noescape` may not escape.
                    let canonical = self.canonical_type_ref(return_ty);
                    self.check_unsupported_syntax_type_ref(&canonical, false, true);
                }
                self.check_unsupported_syntax_block(&function.body);
            }
            Item::Type(type_decl) => {
                self.check_supported_derives(&type_decl.derives, &type_decl.span);
                if type_decl.kind == TypeKind::Resource {
                    self.check_resource_derives(&type_decl.derives, &type_decl.span);
                }
                for span in &type_decl.malformed_generic_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed generic parameter declaration",
                        "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                    );
                }
                for span in &type_decl.malformed_field_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed field declaration",
                        "Type fields must use `name: Type`, `name: handle Type`, or `name: weak Type`.",
                    );
                }
                if type_decl.is_opaque && !type_decl.fields.is_empty() {
                    self.unsupported_syntax(
                        type_decl.span.clone(),
                        "unsupported opaque type body",
                        "Opaque interface types hide their representation. Declare `opaque struct Name`, `opaque class Name`, or `opaque resource Name` without fields.",
                    );
                }
                if type_decl.is_opaque && type_decl.drop_body.is_some() {
                    self.unsupported_syntax(
                        type_decl.span.clone(),
                        "unsupported opaque type body",
                        "Opaque resource contracts hide their implementation details, including drop bodies. Resource cleanup belongs to the implementation, not the `.rssi` contract.",
                    );
                }
                for field in &type_decl.fields {
                    // Struct/class fields are storable positions: an `owned Fn`
                    // field is first-class; `noescape` fields are rejected.
                    let canonical = self.canonical_type_ref(&field.ty);
                    self.check_unsupported_syntax_type_ref(&canonical, false, true);
                }
                if type_decl.kind != TypeKind::Resource
                    && let Some(drop_body) = &type_decl.drop_body
                {
                    self.unsupported_syntax(
                        drop_body.span.clone(),
                        "unsupported managed drop",
                        "Managed class and struct values do not have user-observable destructors in v0.7. Use `resource` with `with` for deterministic cleanup.",
                    );
                }
            }
            Item::SumType(sum) => {
                self.check_supported_derives(&sum.derives, &sum.span);
                for field in sum.variants.iter().flat_map(|variant| &variant.fields) {
                    let canonical = self.canonical_type_ref(&field.ty);
                    self.check_unsupported_syntax_type_ref(&canonical, false, true);
                }
            }
            Item::Const(decl) => {
                // v0.7 `const` initializers must be literals (mirroring
                // `lower_const_value`). Reject anything else with a stable
                // diagnostic instead of lowering it to a `()` placeholder, which
                // produced an unmappable backend type error (RS1102/E0308).
                let is_literal = matches!(
                    &decl.value,
                    Expr::Number(_, _) | Expr::String(_, _) | Expr::MultilineString(_, _)
                ) || matches!(&decl.value, Expr::Ident(name, _) if name == "true" || name == "false");
                if !is_literal {
                    self.unsupported_syntax(
                        decl.span.clone(),
                        "unsupported const initializer",
                        "A v0.7 `const` initializer must be a literal (number, string, or `true`/`false`). Compute the value and write it as a literal; expressions and calls in `const` position are not supported yet.",
                    );
                }
            }
            Item::Module(_) | Item::Use(_) | Item::TypeAlias(_) => {}
        }
    }

    /// Validate `owned`/`noescape` Fn placement.
    ///
    /// Soundness boundary (RSS principle):
    /// - `owned Fn(...)` is a FIRST-CLASS value: allowed as a direct parameter
    ///   AND in storable positions (generic argument, struct field, `let`/
    ///   `local` binding type, function return type). A stored/escaping owned
    ///   closure may only capture owned (move) or `Copy` values, so it cannot
    ///   dangle — the capture-soundness checks elsewhere enforce that.
    /// - `noescape Fn(...)` stays PARAMETER-ONLY. A noescape callback may
    ///   borrow-capture, so letting it be stored/returned would let a borrow
    ///   escape. It is rejected anywhere except a direct function parameter.
    ///
    /// `allow_noescape` is true only at a direct parameter position and never
    /// propagates into nested type positions. `allow_owned` is true at a
    /// parameter position and at every storable position, and propagates into
    /// nested positions (a `List<owned Fn(...)>`, an `owned Fn` field, an
    /// `owned Fn` return, or an `owned Fn` returning/taking another `owned Fn`).
    pub(super) fn check_unsupported_syntax_type_ref(
        &mut self,
        ty: &TypeRef,
        allow_noescape: bool,
        allow_owned: bool,
    ) {
        if ty.is_noescape && (!allow_noescape || ty.name != "Fn") {
            self.unsupported_syntax(
                ty.span.clone(),
                "unsupported noescape position",
                "`noescape Fn(...)` is only supported as a direct function parameter type.",
            );
        }
        if ty.is_owned && (!allow_owned || ty.name != "Fn") {
            self.unsupported_syntax(
                ty.span.clone(),
                "unsupported owned position",
                "`owned Fn(...)` is supported as a function parameter and in storable positions (generic argument, struct field, binding, or return type).",
            );
        }
        for span in &ty.malformed_arg_spans {
            self.unsupported_syntax(
                span.clone(),
                "malformed type argument",
                "Type arguments must be valid type references; empty or unsupported type argument slots are not allowed.",
            );
        }
        // `owned` Fn stays first-class through nested positions; `noescape`
        // never does (it is strictly a direct-parameter capability).
        for arg in &ty.args {
            self.check_unsupported_syntax_type_ref(arg, false, allow_owned);
        }
        for param in &ty.fn_params {
            self.check_unsupported_syntax_type_ref(param, false, allow_owned);
        }
        if let Some(ret) = &ty.fn_return {
            self.check_unsupported_syntax_type_ref(ret, false, allow_owned);
        }
    }

    pub(super) fn check_unsupported_syntax_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.check_unsupported_syntax_stmt(statement);
        }
    }

    pub(super) fn check_unsupported_syntax_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.malformed {
                    self.unsupported_syntax(
                        stmt.span.clone(),
                        "malformed statement",
                        "`let` and `local` bindings need a binding name, and an `=` must be followed by an expression.",
                    );
                }
               if stmt.is_async && !self.in_task_group {
                   self.unsupported_syntax(
                       stmt.span.clone(),
                       "`async let` outside task_group",
                       "`async let` can only be used inside a `task_group { ... }` block.",
                   );
               }
               if let Some(ty) = &stmt.type_annotation {
                   let canonical = self.canonical_type_ref(ty);
                   self.check_unsupported_syntax_type_ref(&canonical, false, true);
               }
               if let Some(value) = &stmt.value {
                   self.check_unsupported_syntax_expr(value);
               }
           }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_unsupported_syntax_expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.resource);
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedWith(span) => self.unsupported_syntax(
                span.clone(),
                "malformed with statement",
                "`with` statements must use `with resource as name { ... }`.",
            ),
            Stmt::If(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.condition);
                self.check_unsupported_syntax_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_unsupported_syntax_block(else_body);
                }
            }
            Stmt::MalformedIf(span) => self.unsupported_syntax(
                span.clone(),
                "malformed if statement",
                "`if` statements must use `if condition { ... }` with optional `else { ... }` or `else if ...`.",
            ),
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_unsupported_syntax_expr(condition);
                }
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedLoop(span) => self.unsupported_syntax(
                span.clone(),
                "malformed loop statement",
                "`loop` statements must use `loop { ... }`; `while` statements must use `while condition { ... }`.",
            ),
            Stmt::For(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.iterable);
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_task_group_async_let_shape(&stmt.body);
                self.check_task_group_async_lets_consumed(&stmt.body);
                let was_in_task_group = self.in_task_group;
                self.in_task_group = true;
                self.check_unsupported_syntax_block(&stmt.body);
                self.in_task_group = was_in_task_group;
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    if async_await_inner_ast(&arm.operation).is_none() {
                        self.unsupported_syntax(
                            arm.span.clone(),
                            "malformed select arm",
                            "Select arms must use `name = await operation => { ... }`.",
                        );
                    }
                    self.check_unsupported_syntax_expr(&arm.operation);
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Stmt::MalformedFor(span) => self.unsupported_syntax(
                span.clone(),
                "malformed for statement",
                "`for` statements must use `for name in iterable { ... }`.",
            ),
            Stmt::Match(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.value);
                for span in &stmt.malformed_arm_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed match arm",
                        "Match arms must use `pattern => statement` or `pattern => { ... }`.",
                    );
                }
                for arm in &stmt.arms {
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.value);
                self.check_unsupported_syntax_block(&stmt.else_body);
            }
            Stmt::MalformedMatch(span) => self.unsupported_syntax(
                span.clone(),
                "malformed match statement",
                "`match` statements must use `match value { pattern => ... }`.",
            ),
            Stmt::Assign(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.target);
                self.check_unsupported_syntax_expr(&stmt.value);
            }
            Stmt::Expr(expr) => self.check_unsupported_syntax_expr(expr),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(span) => self.unsupported_syntax(
                span.clone(),
                "unsupported statement",
                "This statement is outside the current RSScript parser surface.",
            ),
        }
    }

    pub(super) fn check_unsupported_syntax_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.check_unsupported_syntax_expr(left);
                self.check_unsupported_syntax_expr(right);
            }
            Expr::Field { base, .. } => self.check_unsupported_syntax_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_unsupported_syntax_expr(base);
                self.check_unsupported_syntax_expr(index);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    if arg.malformed {
                        self.unsupported_syntax(
                            arg.span.clone(),
                            "malformed call argument",
                            "Call arguments cannot contain empty argument slots.",
                        );
                    } else {
                        self.check_unsupported_syntax_expr(&arg.value);
                    }
                }
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Spawn { value, span } => {
                self.unsupported_syntax(
                    span.clone(),
                    "unsupported spawn expression",
                    "`spawn` is not a v0.7 source-level task feature. Use `task_group { async let ... }` for structured isolate-local async work.",
                );
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Await { value, .. } => {
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Closure { body, .. } => self.check_unsupported_syntax_block(body),
            Expr::Match {
                value,
                arms,
                malformed_arm_spans,
                ..
            } => {
                self.check_unsupported_syntax_expr(value);
                for span in malformed_arm_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed match arm",
                        "Match arms must use `pattern => statement` or `pattern => { ... }`.",
                    );
                }
                for arm in arms {
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Expr::ObjectLiteral { fields, .. } => {
                for field in fields {
                    self.check_unsupported_syntax_expr(&field.value);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_unsupported_syntax_expr(&entry.key);
                    self.check_unsupported_syntax_expr(&entry.value);
                }
            }
            Expr::ArrayLiteral { items, .. } => {
                for item in items {
                    self.check_unsupported_syntax_expr(item);
                }
            }
            Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::CharLiteral(_, _)
            | Expr::MultilineString(_, _) => {}
            Expr::Unknown(span) => {
                self.unsupported_syntax(
                    span.clone(),
                    "unsupported expression",
                    "This expression is outside the current RSScript parser surface.",
                );
            }
        }
    }

    pub(super) fn check_task_group_async_lets_consumed(&mut self, block: &Block) {
        let mut async_lets = Vec::new();
        let mut awaited = HashSet::new();
        collect_task_group_async_lets(block, &mut async_lets);
        collect_direct_task_group_awaited_handles(block, &mut awaited);
        for (name, span) in async_lets {
            if name == "_" {
                continue;
            }
            if !awaited.contains(&name) {
                self.unsupported_syntax(
                    span,
                    "unawaited async let",
                    "`async let` handles are lexical task_group handles and must be consumed by `await` inside the same `task_group { ... }` block.",
                );
            }
        }
    }

    pub(super) fn check_task_group_async_let_shape(&mut self, block: &Block) {
        let mut top_level_async_lets = HashSet::new();
        for statement in &block.statements {
            if let Stmt::Let(stmt) = statement
                && stmt.is_async
                && stmt.name != "_"
            {
                top_level_async_lets.insert(stmt.name.clone());
            }
        }

        let mut nested_async_lets = Vec::new();
        collect_nested_task_group_async_lets(block, &mut nested_async_lets);
        for span in nested_async_lets {
            self.unsupported_syntax(
                span,
                "nested async let",
                "`async let` is currently supported only as a direct child of `task_group { ... }` so checking and lowering share one structured-concurrency model.",
            );
        }

        let mut all_awaited = HashSet::new();
        let mut direct_awaited = HashSet::new();
        collect_task_group_awaited_handles(block, &mut all_awaited);
        collect_direct_task_group_awaited_handles(block, &mut direct_awaited);
        for name in all_awaited {
            if top_level_async_lets.contains(&name) && !direct_awaited.contains(&name) {
                let span = find_nested_task_group_await_span(block, &name)
                    .cloned()
                    .unwrap_or_else(|| block.span.clone());
                self.unsupported_syntax(
                    span,
                    "nested async let await",
                    "`await` of a task_group async-let handle must be a direct task_group statement in the v0.7 executable MVP.",
                );
            }
        }

        let mut declared = HashSet::new();
        let mut awaited = HashSet::new();
        for statement in &block.statements {
            if let Stmt::Let(stmt) = statement
                && stmt.is_async
            {
                if stmt.name != "_" {
                    declared.insert(stmt.name.clone());
                }
                continue;
            }

            for (name, span) in direct_task_group_awaited_handles_in_stmt(statement) {
                if !top_level_async_lets.contains(&name) {
                    continue;
                }
                if !declared.contains(&name) {
                    self.unsupported_syntax(
                        span,
                        "async let await before declaration",
                        "`await` of a task_group async-let handle must appear after the matching `async let` declaration.",
                    );
                } else if !awaited.insert(name) {
                    self.unsupported_syntax(
                        span,
                        "async let awaited more than once",
                        "`async let` handles are bounded task_group handles and can be consumed by `await` only once.",
                    );
                }
            }
        }
    }

    pub(crate) fn unsupported_syntax(
        &mut self,
        span: crate::diagnostic::Span,
        label: &str,
        cause: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNSUPPORTED_SYNTAX,
                "unsupported RSScript syntax.",
                span,
                label,
            )
            .with_cause(cause)
            .with_fix(
                "rewrite_supported_syntax",
                "Rewrite this construct using the currently supported RSScript syntax.",
                "manual",
            ),
        );
    }
}
