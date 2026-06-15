//! Generated namespace isolation.
//!
//! A file's `module a.b` declaration gives its top-level symbols a module
//! identity. This pass rewrites every module-scoped declaration to a globally
//! unique name (`a_b__name`) and resolves every reference module-aware (local
//! module, then `use` import) so two modules can declare the same source name
//! without colliding after Rust lowering. Files with no `module` declaration are
//! the root namespace and are left completely untouched, so single-module and
//! module-less programs lower exactly as before.
//!
//! The pass runs on the assembled `Program` (after parsing/merging, before HIR
//! construction and lowering) so the checker, the register VM, and the Rust
//! backend all observe the same isolated names.

use std::collections::{HashMap, HashSet};

use super::ast::{
    Block, Callee, ConstDecl, Expr, FunctionDecl, Item, MatchArm, MatchFieldPattern, MatchPattern,
    Program, ProtocolImpl, Stmt, SumTypeDecl, TypeAliasDecl, TypeDecl, TypeRef,
};

/// The separator between a module prefix and a symbol name. Two underscores keep
/// the boundary readable in diagnostics and generated Rust (`a_b__name`).
const MODULE_SEP: &str = "__";

/// Encode a dotted module path as a single Rust-identifier prefix, injectively:
/// each segment's own underscores are doubled before segments are joined with a
/// single underscore, so distinct paths (`a.b` vs `a_b`) never collide.
fn module_prefix(segments: &[String]) -> String {
    segments
        .iter()
        .map(|segment| segment.replace('_', "__"))
        .collect::<Vec<_>>()
        .join("_")
}

/// The module prefix for a dotted namespace string (`a.b` -> `a_b`).
fn module_prefix_from_dotted(namespace: &str) -> String {
    module_prefix(
        &namespace
            .split('.')
            .map(str::to_string)
            .collect::<Vec<_>>(),
    )
}

/// Rewrite a program so each `module`-scoped symbol becomes globally unique.
pub fn isolate_module_namespaces(program: &mut Program) {
    let file_module = collect_file_modules(program);
    if file_module.is_empty() {
        // No `module` declarations anywhere: pure root namespace, nothing to do.
        return;
    }
    let resolver = Resolver::build(program, &file_module);
    resolver.rewrite(program);
}

/// Map each source file to its module prefix (from the file's first `module`
/// declaration). Files without one are absent (root namespace).
fn collect_file_modules(program: &Program) -> HashMap<String, String> {
    let mut file_module = HashMap::new();
    for item in &program.items {
        if let Item::Module(module) = item {
            file_module
                .entry(module.span.file.clone())
                .or_insert_with(|| module_prefix(&module.path));
        }
    }
    file_module
}

struct Resolver {
    /// file -> module prefix.
    file_module: HashMap<String, String>,
    /// module prefix -> the bare names it declares (free fns, free consts, type
    /// roots). These are the names referenceable without a `Type.` qualifier.
    module_defs: HashMap<String, HashSet<String>>,
    /// module prefix -> the type names it declares (for `Type.method` / `Type.X`
    /// namespace resolution).
    module_types: HashMap<String, HashSet<String>>,
    /// module prefix -> the constant names it declares. Constants lower to
    /// `SCREAMING_SNAKE_CASE`, so their mangled symbol is upper-cased to stay
    /// consistent between declaration and reference.
    module_consts: HashMap<String, HashSet<String>>,
    /// module prefix -> the sum-variant names declared by its sums. Used to
    /// resolve qualified value access `module.Variant`; variant names are global
    /// (resolved through their sum type), so qualification rewrites to the bare
    /// variant.
    module_variants: HashMap<String, HashSet<String>>,
    /// file -> (local import name -> (module prefix, real symbol name)). The
    /// local name is the `as` alias when present, otherwise the path's last
    /// segment; the real name is always the path's last segment.
    file_imports: HashMap<String, HashMap<String, (String, String)>>,
}

impl Resolver {
    fn build(program: &Program, file_module: &HashMap<String, String>) -> Self {
        let mut module_defs: HashMap<String, HashSet<String>> = HashMap::new();
        let mut module_types: HashMap<String, HashSet<String>> = HashMap::new();
        let mut module_consts: HashMap<String, HashSet<String>> = HashMap::new();
        let mut module_variants: HashMap<String, HashSet<String>> = HashMap::new();
        let mut file_imports: HashMap<String, HashMap<String, (String, String)>> = HashMap::new();

        for item in &program.items {
            let Some(file) = item_file(item) else {
                continue;
            };
            let Some(prefix) = file_module.get(file) else {
                continue;
            };
            match item {
                Item::Function(function) => {
                    // A free function (no `Type.` qualifier) is referenceable by
                    // its bare name; a method is reached through its type. The
                    // executable entry point `main` stays a global root symbol.
                    if !function.name.contains('.') && function.name != "main" {
                        module_defs
                            .entry(prefix.clone())
                            .or_default()
                            .insert(function.name.clone());
                    }
                }
                Item::Const(decl) => {
                    if !decl.name.contains('.') {
                        module_defs
                            .entry(prefix.clone())
                            .or_default()
                            .insert(decl.name.clone());
                        module_consts
                            .entry(prefix.clone())
                            .or_default()
                            .insert(decl.name.clone());
                    }
                }
                Item::Type(decl) => {
                    module_defs
                        .entry(prefix.clone())
                        .or_default()
                        .insert(decl.name.clone());
                    module_types
                        .entry(prefix.clone())
                        .or_default()
                        .insert(decl.name.clone());
                }
                Item::SumType(decl) => {
                    module_defs
                        .entry(prefix.clone())
                        .or_default()
                        .insert(decl.name.clone());
                    module_types
                        .entry(prefix.clone())
                        .or_default()
                        .insert(decl.name.clone());
                    for variant in &decl.variants {
                        module_variants
                            .entry(prefix.clone())
                            .or_default()
                            .insert(variant.name.clone());
                    }
                }
                Item::TypeAlias(decl) => {
                    module_defs
                        .entry(prefix.clone())
                        .or_default()
                        .insert(decl.name.clone());
                    module_types
                        .entry(prefix.clone())
                        .or_default()
                        .insert(decl.name.clone());
                }
                Item::Use(decl) => {
                    if decl.path.len() >= 2 {
                        let real_name = decl.path[decl.path.len() - 1].clone();
                        let import_prefix = module_prefix(&decl.path[..decl.path.len() - 1]);
                        let local = decl
                            .local_name()
                            .map(str::to_string)
                            .unwrap_or_else(|| real_name.clone());
                        file_imports
                            .entry(decl.span.file.clone())
                            .or_default()
                            .insert(local, (import_prefix, real_name));
                    }
                }
                Item::Module(_) => {}
            }
        }

        Self {
            file_module: file_module.clone(),
            module_defs,
            module_types,
            module_consts,
            module_variants,
            file_imports,
        }
    }

    /// Build the mangled value symbol for `name` declared in module `prefix`,
    /// upper-casing constants so the symbol matches the Rust backend's
    /// `SCREAMING_SNAKE_CASE` const lowering at both declaration and reference.
    fn mangle_value(&self, prefix: &str, name: &str) -> String {
        let mangled = format!("{prefix}{MODULE_SEP}{name}");
        if self
            .module_consts
            .get(prefix)
            .is_some_and(|consts| consts.contains(name))
        {
            mangled.to_uppercase()
        } else {
            mangled
        }
    }

    /// The mangled name for a bare value/type symbol `name` referenced from
    /// `file`, or `None` to leave it unchanged (root symbol, builtin, external,
    /// or unresolved).
    fn resolve_bare(&self, file: &str, name: &str) -> Option<String> {
        // A symbol declared by the reference's own module wins.
        if let Some(prefix) = self.file_module.get(file)
            && self
                .module_defs
                .get(prefix)
                .is_some_and(|names| names.contains(name))
        {
            return Some(self.mangle_value(prefix, name));
        }
        // Otherwise a `use module.name [as local]` import that names a known
        // module symbol. The reference uses the local name; the mangled symbol
        // uses the imported module's real name.
        if let Some((import_prefix, real)) = self
            .file_imports
            .get(file)
            .and_then(|imports| imports.get(name))
            && self
                .module_defs
                .get(import_prefix)
                .is_some_and(|names| names.contains(real))
        {
            return Some(self.mangle_value(import_prefix, real));
        }
        None
    }

    /// The `(module prefix, real type name)` that owns type `name` as seen from
    /// `file` (own module, then `use` import), or `None` if it is not a
    /// module-scoped type. The real name differs from `name` when `name` is a
    /// `use … as` alias.
    fn resolve_type_module(&self, file: &str, name: &str) -> Option<(String, String)> {
        if let Some(prefix) = self.file_module.get(file)
            && self
                .module_types
                .get(prefix)
                .is_some_and(|types| types.contains(name))
        {
            return Some((prefix.clone(), name.to_string()));
        }
        if let Some((import_prefix, real)) = self
            .file_imports
            .get(file)
            .and_then(|imports| imports.get(name))
            && self
                .module_types
                .get(import_prefix)
                .is_some_and(|types| types.contains(real))
        {
            return Some((import_prefix.clone(), real.clone()));
        }
        None
    }

    /// Resolve a `Type.` namespace (for `Type.method` / `Type.CONST`): if the
    /// namespace names a module-scoped type, return its mangled type name.
    fn resolve_type_namespace(&self, file: &str, namespace: &str) -> Option<String> {
        self.resolve_type_module(file, namespace)
            .map(|(prefix, real)| format!("{prefix}{MODULE_SEP}{real}"))
    }

    fn rewrite(&self, program: &mut Program) {
        for item in &mut program.items {
            let Some(file) = item_file(item).map(str::to_string) else {
                continue;
            };
            let in_module = self.file_module.contains_key(&file);
            match item {
                Item::Function(function) => self.rewrite_function(function, &file, in_module),
                Item::Const(decl) => self.rewrite_const(decl, &file, in_module),
                Item::Type(decl) => self.rewrite_type_decl(decl, &file, in_module),
                Item::SumType(decl) => self.rewrite_sum_decl(decl, &file, in_module),
                Item::TypeAlias(decl) => self.rewrite_alias_decl(decl, &file, in_module),
                Item::Module(_) | Item::Use(_) => {}
            }
        }
        for protocol_impl in &mut program.protocol_impls {
            self.rewrite_protocol_impl(protocol_impl);
        }
    }

    fn rewrite_function(&self, function: &mut FunctionDecl, file: &str, in_module: bool) {
        // The executable entry point keeps its global `main` symbol.
        if in_module && function.name != "main" {
            function.name = self.mangle_decl_name(file, &function.name);
        }
        // Parameters bind names that shadow module symbols inside the body.
        let mut scope: HashSet<String> = function
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect();
        for param in &mut function.params {
            self.rewrite_type(&mut param.ty, file);
            if let Some(default) = &mut param.default {
                self.rewrite_expr(default, file, &mut scope);
            }
        }
        if let Some(return_ty) = &mut function.return_ty {
            self.rewrite_type(return_ty, file);
        }
        self.rewrite_block(&mut function.body, file, &mut scope);
    }

    fn rewrite_const(&self, decl: &mut ConstDecl, file: &str, in_module: bool) {
        // Constants are mangled through `mangle_value` (upper-cased) so the
        // declaration matches references and the backend's const lowering.
        if in_module
            && !decl.name.contains('.')
            && let Some(prefix) = self.file_module.get(file)
        {
            decl.name = self.mangle_value(prefix, &decl.name);
        }
        if let Some(ty) = &mut decl.type_annotation {
            self.rewrite_type(ty, file);
        }
        let mut scope = HashSet::new();
        self.rewrite_expr(&mut decl.value, file, &mut scope);
    }

    fn rewrite_type_decl(&self, decl: &mut TypeDecl, file: &str, in_module: bool) {
        if in_module {
            decl.name = self.mangle_decl_name(file, &decl.name);
        }
        for field in &mut decl.fields {
            self.rewrite_type(&mut field.ty, file);
        }
        if let Some(drop_body) = &mut decl.drop_body {
            let mut scope = HashSet::from(["self".to_string()]);
            self.rewrite_block(drop_body, file, &mut scope);
        }
    }

    fn rewrite_sum_decl(&self, decl: &mut SumTypeDecl, file: &str, in_module: bool) {
        if in_module {
            decl.name = self.mangle_decl_name(file, &decl.name);
        }
        for variant in &mut decl.variants {
            for field in &mut variant.fields {
                self.rewrite_type(&mut field.ty, file);
            }
        }
    }

    fn rewrite_alias_decl(&self, decl: &mut TypeAliasDecl, file: &str, in_module: bool) {
        if in_module {
            decl.name = self.mangle_decl_name(file, &decl.name);
        }
        self.rewrite_type(&mut decl.target, file);
    }

    fn rewrite_protocol_impl(&self, protocol_impl: &mut ProtocolImpl) {
        let file = protocol_impl.span.file.clone();
        if let Some(mangled) = self.resolve_type_namespace(&file, &protocol_impl.type_name) {
            protocol_impl.type_name = mangled;
        }
        for mapping in &mut protocol_impl.mappings {
            // A mapping target is a function name (possibly `Type.method`).
            if let Some((ns, method)) = mapping.target.split_once('.') {
                if let Some(mangled_ns) = self.resolve_type_namespace(&file, ns) {
                    mapping.target = format!("{mangled_ns}.{method}");
                }
            } else if let Some(mangled) = self.resolve_bare(&file, &mapping.target) {
                mapping.target = mangled;
            }
        }
    }

    /// Mangle a top-level declaration name owned by a module file: prefix the
    /// (possibly `Type.member`) name with the file's module prefix.
    fn mangle_decl_name(&self, file: &str, name: &str) -> String {
        match self.file_module.get(file) {
            Some(prefix) => format!("{prefix}{MODULE_SEP}{name}"),
            None => name.to_string(),
        }
    }

    fn rewrite_block(&self, block: &mut Block, file: &str, scope: &mut HashSet<String>) {
        let saved: Vec<String> = scope.iter().cloned().collect();
        for stmt in &mut block.statements {
            self.rewrite_stmt(stmt, file, scope);
        }
        // Restore: drop names introduced inside this block.
        scope.clear();
        scope.extend(saved);
    }

    fn rewrite_stmt(&self, stmt: &mut Stmt, file: &str, scope: &mut HashSet<String>) {
        match stmt {
            Stmt::Let(let_stmt) => {
                if let Some(value) = &mut let_stmt.value {
                    self.rewrite_expr(value, file, scope);
                }
                if let Some(ty) = &mut let_stmt.type_annotation {
                    self.rewrite_type(ty, file);
                }
                scope.insert(let_stmt.name.clone());
            }
            Stmt::LetElse(let_else) => {
                self.rewrite_expr(&mut let_else.value, file, scope);
                self.rewrite_pattern(&mut let_else.pattern, file);
                let mut else_scope = scope.clone();
                self.rewrite_block(&mut let_else.else_body, file, &mut else_scope);
                scope.insert(let_else.binding_name.clone());
            }
            Stmt::Return(ret) => {
                if let Some(value) = &mut ret.value {
                    self.rewrite_expr(value, file, scope);
                }
            }
            Stmt::With(with_stmt) => {
                self.rewrite_expr(&mut with_stmt.resource, file, scope);
                let mut inner = scope.clone();
                inner.insert(with_stmt.binding.clone());
                self.rewrite_block(&mut with_stmt.body, file, &mut inner);
            }
            Stmt::If(if_stmt) => {
                self.rewrite_expr(&mut if_stmt.condition, file, scope);
                let mut then_scope = scope.clone();
                self.rewrite_block(&mut if_stmt.then_body, file, &mut then_scope);
                if let Some(else_body) = &mut if_stmt.else_body {
                    let mut else_scope = scope.clone();
                    self.rewrite_block(else_body, file, &mut else_scope);
                }
            }
            Stmt::Loop(loop_stmt) => {
                if let Some(condition) = &mut loop_stmt.condition {
                    self.rewrite_expr(condition, file, scope);
                }
                let mut inner = scope.clone();
                self.rewrite_block(&mut loop_stmt.body, file, &mut inner);
            }
            Stmt::For(for_stmt) => {
                self.rewrite_expr(&mut for_stmt.iterable, file, scope);
                let mut inner = scope.clone();
                inner.insert(for_stmt.binding.clone());
                self.rewrite_block(&mut for_stmt.body, file, &mut inner);
            }
            Stmt::Match(match_stmt) => {
                self.rewrite_expr(&mut match_stmt.value, file, scope);
                for arm in &mut match_stmt.arms {
                    self.rewrite_match_arm(arm, file, scope);
                }
            }
            Stmt::TaskGroup(task_group) => {
                let mut inner = scope.clone();
                self.rewrite_block(&mut task_group.body, file, &mut inner);
            }
            Stmt::Select(select) => {
                for arm in &mut select.arms {
                    self.rewrite_expr(&mut arm.operation, file, scope);
                    let mut inner = scope.clone();
                    inner.insert(arm.binding.clone());
                    self.rewrite_block(&mut arm.body, file, &mut inner);
                }
            }
            Stmt::Assign(assign) => {
                self.rewrite_expr(&mut assign.target, file, scope);
                self.rewrite_expr(&mut assign.value, file, scope);
            }
            Stmt::Expr(expr) => self.rewrite_expr(expr, file, scope),
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

    fn rewrite_match_arm(&self, arm: &mut MatchArm, file: &str, scope: &mut HashSet<String>) {
        self.rewrite_pattern(&mut arm.pattern, file);
        let mut inner = scope.clone();
        for name in arm.pattern.binding_names() {
            inner.insert(name.to_string());
        }
        if let Some(guard) = &mut arm.guard {
            self.rewrite_expr(guard, file, &mut inner);
        }
        self.rewrite_block(&mut arm.body, file, &mut inner);
    }

    /// Rewrite constructor names in patterns (`Foo { .. }` / `Foo(..)`) when they
    /// name a module-scoped type. Built-in variants (`Some`/`Ok`/...) never match
    /// a user type and are left alone.
    fn rewrite_pattern(&self, pattern: &mut MatchPattern, file: &str) {
        match pattern {
            MatchPattern::Variant { name, binding, .. } => {
                if let Some(mangled) = self.resolve_bare(file, name) {
                    *name = mangled;
                }
                if let Some(binding) = binding {
                    self.rewrite_pattern(binding, file);
                }
            }
            MatchPattern::Struct { name, fields, .. } => {
                if let Some(mangled) = self.resolve_bare(file, name) {
                    *name = mangled;
                }
                for field in fields {
                    self.rewrite_field_pattern(field, file);
                }
            }
            MatchPattern::List {
                prefix, suffix, ..
            } => {
                for nested in prefix.iter_mut().chain(suffix) {
                    self.rewrite_pattern(nested, file);
                }
            }
            MatchPattern::Binding { .. }
            | MatchPattern::Literal { .. }
            | MatchPattern::Wildcard(_) => {}
        }
    }

    fn rewrite_field_pattern(&self, field: &mut MatchFieldPattern, file: &str) {
        if let Some(pattern) = &mut field.pattern {
            self.rewrite_pattern(pattern, file);
        }
    }

    fn rewrite_expr(&self, expr: &mut Expr, file: &str, scope: &mut HashSet<String>) {
        match expr {
            Expr::Ident(name, _) => {
                if !scope.contains(name)
                    && let Some(mangled) = self.resolve_bare(file, name)
                {
                    *name = mangled;
                }
            }
            Expr::Call { callee, args, .. } => {
                self.rewrite_callee(callee, file, scope);
                for arg in args {
                    self.rewrite_expr(&mut arg.value, file, scope);
                }
            }
            Expr::Field { base, name, span } => {
                // `Type.CONST` associated-const access: when the base is a
                // module-scoped type and that module declares the (per-file
                // flattened) associated constant, resolve the whole access to the
                // module-qualified const symbol. This handles the cross-file case,
                // where the access lives in a different file than the declaration.
                if let Expr::Ident(type_name, _) = base.as_ref()
                    && !scope.contains(type_name)
                    && let Some((type_module, real_type)) = self.resolve_type_module(file, type_name)
                {
                    let flat = flatten_associated_const(&real_type, name);
                    if self
                        .module_defs
                        .get(&type_module)
                        .is_some_and(|names| names.contains(&flat))
                    {
                        *expr = Expr::Ident(self.mangle_value(&type_module, &flat), span.clone());
                        return;
                    }
                }
                // `module.CONST` / `module.Variant` qualified value access: when
                // the base is a module path (not a local) that declares the named
                // constant or sum variant, resolve the whole access. A constant
                // becomes its module-mangled symbol; a variant becomes the bare
                // variant (variant names resolve globally through their sum type).
                if let Some(prefix) = module_path_of_receiver(base, scope) {
                    if self
                        .module_consts
                        .get(&prefix)
                        .is_some_and(|consts| consts.contains(name))
                    {
                        *expr = Expr::Ident(self.mangle_value(&prefix, name), span.clone());
                        return;
                    }
                    if self
                        .module_variants
                        .get(&prefix)
                        .is_some_and(|variants| variants.contains(name))
                    {
                        *expr = Expr::Ident(name.clone(), span.clone());
                        return;
                    }
                }
                self.rewrite_expr(base, file, scope);
            }
            Expr::Index { base, index, .. } => {
                self.rewrite_expr(base, file, scope);
                self.rewrite_expr(index, file, scope);
            }
            Expr::Binary { left, right, .. } => {
                self.rewrite_expr(left, file, scope);
                self.rewrite_expr(right, file, scope);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => self.rewrite_expr(value, file, scope),
            Expr::ObjectLiteral { fields, .. } => {
                for field in fields {
                    self.rewrite_expr(&mut field.value, file, scope);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.rewrite_expr(&mut entry.key, file, scope);
                    self.rewrite_expr(&mut entry.value, file, scope);
                }
            }
            Expr::ArrayLiteral { items, .. } => {
                for item in items {
                    self.rewrite_expr(item, file, scope);
                }
            }
            Expr::Closure { params, body, .. } => {
                let mut inner = scope.clone();
                for param in params.iter() {
                    inner.insert(param.clone());
                }
                self.rewrite_block(body, file, &mut inner);
            }
            Expr::Match {
                value, arms, ..
            } => {
                self.rewrite_expr(value, file, scope);
                for arm in arms {
                    self.rewrite_match_arm(arm, file, scope);
                }
            }
            Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    fn rewrite_callee(&self, callee: &mut Callee, file: &str, scope: &mut HashSet<String>) {
        match callee {
            Callee::Name(name) => {
                if !scope.contains(name)
                    && let Some(mangled) = self.resolve_bare(file, name)
                {
                    *name = mangled;
                }
            }
            Callee::Qualified { namespace, name } => {
                if scope.contains(namespace.as_str()) {
                    return;
                }
                // `Type.method` / `Type.CONST`: rewrite the type namespace, keep
                // the member name.
                if let Some(mangled) = self.resolve_type_namespace(file, namespace) {
                    *namespace = mangled;
                    return;
                }
                // `module.function`: a module-qualified free call collapses to the
                // module's mangled function symbol.
                let prefix = module_prefix_from_dotted(namespace);
                if self
                    .module_defs
                    .get(&prefix)
                    .is_some_and(|names| names.contains(name))
                {
                    *callee = Callee::Name(format!("{prefix}{MODULE_SEP}{name}"));
                }
            }
            Callee::ReceiverCall {
                receiver, method, ..
            } => {
                // A lowercase `module.fn()` parses as a receiver call, but if the
                // receiver names a module (not a local in scope) whose free
                // function is `method`, it is a module-qualified call: collapse it
                // to the module's mangled symbol. Otherwise it is a real value
                // receiver call and only the receiver is rewritten.
                if let Some(prefix) = module_path_of_receiver(receiver, scope)
                    && self
                        .module_defs
                        .get(&prefix)
                        .is_some_and(|names| names.contains(method))
                {
                    *callee = Callee::Name(format!("{prefix}{MODULE_SEP}{method}"));
                    return;
                }
                self.rewrite_expr(receiver, file, scope);
            }
        }
    }

    fn rewrite_type(&self, ty: &mut TypeRef, file: &str) {
        // Leave single-letter generic parameters and built-ins alone; only names
        // that resolve to a module-scoped type are rewritten.
        if let Some(mangled) = self.resolve_type_namespace(file, &ty.name) {
            ty.name = mangled;
        } else if let Some((namespace, type_name)) = ty.name.rsplit_once('.') {
            // A qualified `module.Type` reference (`dtype.DType`): if `module`
            // names a module that declares `Type`, mangle to its module-scoped
            // type symbol. This lets a type-defining module be referenced without
            // a `use` line in every consuming file.
            let prefix = module_prefix_from_dotted(namespace);
            if self
                .module_types
                .get(&prefix)
                .is_some_and(|types| types.contains(type_name))
            {
                ty.name = format!("{prefix}{MODULE_SEP}{type_name}");
            }
        }
        for arg in &mut ty.args {
            self.rewrite_type(arg, file);
        }
        for param in &mut ty.fn_params {
            self.rewrite_type(param, file);
        }
        if let Some(return_ty) = &mut ty.fn_return {
            self.rewrite_type(return_ty, file);
        }
    }
}

/// Rewrite module-mangled symbol names back to their RSScript source identity
/// (`helpers.count`) inside a diagnostic set, so messages name the source symbol
/// rather than the lowered Rust name. A no-op for programs without `module`
/// declarations (the reconstructed map is empty).
pub fn demangle_diagnostics(program: &Program, diagnostics: &mut [crate::diagnostic::Diagnostic]) {
    let file_module = collect_file_modules(program);
    if file_module.is_empty() {
        return;
    }
    // file -> dotted module display (`a.b.c`).
    let mut file_display: HashMap<String, String> = HashMap::new();
    for item in &program.items {
        if let Item::Module(module) = item {
            file_display
                .entry(module.span.file.clone())
                .or_insert_with(|| module.path.join("."));
        }
    }
    // Reconstruct mangled -> source-display from the isolated declarations.
    let mut reverse: Vec<(String, String)> = Vec::new();
    for item in &program.items {
        let (Some(file), Some(name)) = (item_file(item), item_decl_name(item)) else {
            continue;
        };
        let Some(prefix) = file_module.get(file) else {
            continue;
        };
        let marker = format!("{prefix}{MODULE_SEP}");
        if let Some(original) = name.strip_prefix(&marker) {
            let dotted = file_display.get(file).cloned().unwrap_or_default();
            reverse.push((name.to_string(), format!("{dotted}.{original}")));
        }
    }
    // Replace longer mangled names first so a shorter name is never a prefix of a
    // longer one mid-substitution.
    reverse.sort_by_key(|(mangled, _)| std::cmp::Reverse(mangled.len()));
    if reverse.is_empty() {
        return;
    }
    for diagnostic in diagnostics.iter_mut() {
        for (mangled, source) in &reverse {
            if diagnostic.summary.contains(mangled.as_str()) {
                diagnostic.summary = diagnostic.summary.replace(mangled.as_str(), source);
            }
            if diagnostic.label.contains(mangled.as_str()) {
                diagnostic.label = diagnostic.label.replace(mangled.as_str(), source);
            }
            for cause in &mut diagnostic.causes {
                if cause.contains(mangled.as_str()) {
                    *cause = cause.replace(mangled.as_str(), source);
                }
            }
        }
    }
}

fn item_decl_name(item: &Item) -> Option<&str> {
    match item {
        Item::Function(decl) => Some(decl.name.as_str()),
        Item::Const(decl) => Some(decl.name.as_str()),
        Item::Type(decl) => Some(decl.name.as_str()),
        Item::SumType(decl) => Some(decl.name.as_str()),
        Item::TypeAlias(decl) => Some(decl.name.as_str()),
        Item::Module(_) | Item::Use(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::merge_programs;
    use crate::syntax::parse_source;

    fn function_names(program: &Program) -> Vec<String> {
        program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn root_module_files_are_untouched() {
        // No `module` declaration anywhere: pure root namespace, no rewriting.
        let mut program = parse_source("a.rss", "fn count() -> Int {\n    return 1\n}\n");
        let before = function_names(&program);
        isolate_module_namespaces(&mut program);
        assert_eq!(function_names(&program), before);
    }

    #[test]
    fn module_files_qualify_symbols_and_resolve_local_references() {
        let helpers = parse_source(
            "helpers.rss",
            "module helpers\n\nfn count() -> Int {\n    return 1\n}\n\nfn doubled() -> Int {\n    return count()\n}\n",
        );
        let device = parse_source(
            "device.rss",
            "module device\n\nfn count() -> Int {\n    return 2\n}\n",
        );
        let mut program = merge_programs([helpers, device]);
        isolate_module_namespaces(&mut program);
        let names = function_names(&program);
        // Same source name `count` coexists as two module-qualified symbols.
        assert!(names.contains(&"helpers__count".to_string()), "{names:?}");
        assert!(names.contains(&"device__count".to_string()), "{names:?}");
        // A local reference resolves to the same module's symbol.
        let doubled = program
            .items
            .iter()
            .find_map(|item| match item {
                Item::Function(function) if function.name == "helpers__doubled" => Some(function),
                _ => None,
            })
            .expect("helpers.doubled present");
        let body = format!("{:?}", doubled.body);
        assert!(
            body.contains("helpers__count"),
            "local call should resolve to helpers__count: {body}"
        );
    }

    #[test]
    fn module_prefix_is_injective_across_dotted_and_underscored_paths() {
        // `a.b` and `a_b` are distinct module paths and must not share a prefix.
        let dotted = module_prefix(&["a".to_string(), "b".to_string()]);
        let underscored = module_prefix(&["a_b".to_string()]);
        assert_ne!(dotted, underscored);
        assert_eq!(dotted, "a_b");
        assert_eq!(underscored, "a__b");
        assert_eq!(module_prefix_from_dotted("a.b"), "a_b");
    }

    #[test]
    fn two_modules_with_colliding_join_keep_distinct_symbols() {
        let ab = parse_source("ab.rss", "module a.b\n\nfn count() -> Int {\n    return 1\n}\n");
        let a_b = parse_source("a_b.rss", "module a_b\n\nfn count() -> Int {\n    return 2\n}\n");
        let mut program = merge_programs([ab, a_b]);
        isolate_module_namespaces(&mut program);
        let names = function_names(&program);
        assert!(names.contains(&"a_b__count".to_string()), "{names:?}");
        assert!(names.contains(&"a__b__count".to_string()), "{names:?}");
    }

    #[test]
    fn entry_point_main_is_never_qualified() {
        let mut program = parse_source(
            "main.rss",
            "module app\n\nfn main() -> Unit {\n    return Unit\n}\n",
        );
        isolate_module_namespaces(&mut program);
        assert!(function_names(&program).contains(&"main".to_string()));
    }

    fn body_of(program: &Program, name: &str) -> String {
        let function = program
            .items
            .iter()
            .find_map(|item| match item {
                Item::Function(function) if function.name == name => Some(function),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} present"));
        format!("{:?}", function.body)
    }

    #[test]
    fn aliased_import_resolves_to_the_imported_module_symbol() {
        // `use lib.count as lib_count` lets `app` reference `lib`'s `count` under
        // a distinct local name; the reference resolves to the real module symbol.
        let lib = parse_source(
            "lib.rss",
            "module lib\n\nfn count() -> Int {\n    return 1\n}\n",
        );
        let app = parse_source(
            "app.rss",
            "module app\n\nuse lib.count as lib_count\n\nfn run() -> Int {\n    return lib_count()\n}\n",
        );
        let mut program = merge_programs([lib, app]);
        isolate_module_namespaces(&mut program);
        let body = body_of(&program, "app__run");
        assert!(
            body.contains("lib__count"),
            "alias should resolve to lib__count: {body}"
        );
    }

    #[test]
    fn qualified_module_call_resolves_to_the_module_symbol() {
        // `helpers.count()` names the owner at the call site, with no `use`.
        let helpers = parse_source(
            "helpers.rss",
            "module helpers\n\nfn count() -> Int {\n    return 1\n}\n",
        );
        let app = parse_source(
            "app.rss",
            "module app\n\nfn run() -> Int {\n    return helpers.count()\n}\n",
        );
        let mut program = merge_programs([helpers, app]);
        isolate_module_namespaces(&mut program);
        let body = body_of(&program, "app__run");
        assert!(
            body.contains("helpers__count"),
            "qualified call should resolve to helpers__count: {body}"
        );
    }

    #[test]
    fn multi_segment_qualified_module_call_resolves() {
        // A dotted module path (`a.b`) is mangled to `a_b` and the call collapses
        // to its module-qualified symbol.
        let inner = parse_source(
            "inner.rss",
            "module a.b\n\nfn count() -> Int {\n    return 1\n}\n",
        );
        let app = parse_source(
            "app.rss",
            "module app\n\nfn run() -> Int {\n    return a.b.count()\n}\n",
        );
        let mut program = merge_programs([inner, app]);
        isolate_module_namespaces(&mut program);
        let body = body_of(&program, "app__run");
        assert!(
            body.contains("a_b__count"),
            "multi-segment qualified call should resolve to a_b__count: {body}"
        );
    }
}

/// The flattened symbol for an associated constant `Type.MEMBER`, matching the
/// `desugar::associated_consts` flattening (`.` -> `_`, upper-cased) so isolation
/// resolves to the same constant the per-file desugar produced.
fn flatten_associated_const(type_name: &str, member: &str) -> String {
    format!("{type_name}_{member}").to_uppercase()
}

/// If `receiver` is a chain of plain identifiers (`a` or `a.b.c`) whose root is
/// not shadowed by a local binding, return the candidate module prefix (encoded
/// with the same injective scheme as declarations). Anything else (a method call,
/// an indexed value, a literal, or a root that names an in-scope local) is a
/// value receiver, not a module path.
fn module_path_of_receiver(receiver: &Expr, scope: &HashSet<String>) -> Option<String> {
    fn collect(expr: &Expr, segments: &mut Vec<String>) -> bool {
        match expr {
            Expr::Ident(name, _) => {
                segments.push(name.clone());
                true
            }
            Expr::Field { base, name, .. } => {
                collect(base, segments) && {
                    segments.push(name.clone());
                    true
                }
            }
            _ => false,
        }
    }
    let mut segments = Vec::new();
    if !collect(receiver, &mut segments) || segments.is_empty() {
        return None;
    }
    if scope.contains(&segments[0]) {
        return None;
    }
    Some(module_prefix(&segments))
}

fn item_file(item: &Item) -> Option<&str> {
    Some(match item {
        Item::Function(function) => function.span.file.as_str(),
        Item::Const(decl) => decl.span.file.as_str(),
        Item::Type(decl) => decl.span.file.as_str(),
        Item::SumType(decl) => decl.span.file.as_str(),
        Item::TypeAlias(decl) => decl.span.file.as_str(),
        Item::Module(decl) => decl.span.file.as_str(),
        Item::Use(decl) => decl.span.file.as_str(),
    })
}
