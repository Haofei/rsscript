use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Span;
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, Expr, FieldDecl, ForStmt, GenericParam, Item,
    LetStmt, MatchPattern, MatchStmt, Param, Program, Stmt, TypeKind, TypeRef,
};

use super::helpers::*;
use super::intrinsics::*;
use super::types::{LoweredRust, RustSourceMapEntry};

pub(super) struct RustLowerer<'a> {
    pub(super) program: &'a Program,
    pub(super) type_kinds: BTreeMap<String, TypeKind>,
    pub(super) protocol_names: BTreeSet<String>,
    pub(super) native_boundary_callees: BTreeSet<String>,
    pub(super) async_native_boundary_callees: BTreeSet<String>,
    pub(super) native_bindings: BTreeMap<String, String>,
    pub(super) function_return_types: BTreeMap<String, TypeRef>,
    pub(super) function_type_params: BTreeMap<String, Vec<String>>,
    pub(super) function_param_types: BTreeMap<String, Vec<(String, TypeRef)>>,
    pub(super) function_param_defaults: BTreeMap<String, Vec<Option<Expr>>>,
    pub(super) function_param_effects: BTreeMap<String, Vec<(String, Option<DataEffect>)>>,
    pub(super) retained_params_by_callee: BTreeMap<String, BTreeSet<String>>,
    pub(super) param_effects: BTreeMap<String, DataEffect>,
    pub(super) value_types: BTreeMap<String, TypeRef>,
    pub(super) managed_bindings: BTreeSet<String>,
    pub(super) read_view_bindings: BTreeSet<String>,
    pub(super) current_retained_params: BTreeSet<String>,
    pub(super) mutated_bindings: BTreeSet<String>,
    pub(super) drop_field_names: BTreeSet<String>,
    pub(super) current_return_type: Option<TypeRef>,
    pub(super) current_async_executor: Option<String>,
    /// Name of the enclosing `task_group`'s cancellation guard local, if any, so
    /// `Task.cancellation_token()` resolves to that scope's token.
    pub(super) current_task_group_token: Option<String>,
    pub(super) source_map: Vec<RustSourceMapEntry>,
    /// Maps generic type param name -> protocol bound name for receiver-call resolution
    pub(super) generic_protocol_bounds: BTreeMap<String, String>,
}

pub(super) struct AsyncTaskGroupBoundary {
    pub(super) pending: String,
    pub(super) returns_result: bool,
}

impl<'a> RustLowerer<'a> {
    pub(super) fn new(
        program: &'a Program,
        native_bindings: BTreeMap<String, String>,
        interface_programs: &[Program],
    ) -> Self {
        // Type kinds (struct/class/resource) for `is_class_type`/`is_resource_type`,
        // constructor lowering, etc. Built from the current program *and* dependency
        // interfaces, so a class/resource/struct defined in another package and
        // constructed/held in this source is classified correctly (otherwise it
        // falls through to the unknown-type path and mis-lowers, e.g. a named-field
        // class constructed as a positional `Widget(1)` call — review #8).
        //
        // Bundled stdlib interface types are EXCLUDED: they declare runtime-backed
        // types (e.g. `ProcessRequest`, lowered as `rsscript_runtime::ProcessRequest`)
        // as plain structs, and classifying those as local user types would drop the
        // `rsscript_runtime::` qualification. So only *non-builtin* (dependency)
        // interface types are ingested; the current program wins on any conflict.
        let builtin_type_names = builtin_interface_type_names();
        let type_kinds = interface_programs
            .iter()
            .flat_map(|interface| interface.items.iter())
            .filter(|item| match item {
                Item::Type(ty) => !builtin_type_names.contains(ty.name.as_str()),
                _ => false,
            })
            .chain(program.items.iter())
            .filter_map(|item| match item {
                Item::Type(ty) => Some((ty.name.clone(), ty.kind)),
                Item::SumType(_) => None, // sum types don't contribute to type_kinds map
                Item::Function(_)
                | Item::TypeAlias(_)
                | Item::Const(_)
                | Item::Module(_)
                | Item::Use(_) => None,
            })
            .collect();
        let native_boundary_callees = collect_native_boundary_callees(program, interface_programs);
        let async_native_boundary_callees =
            collect_async_native_boundary_callees(program, interface_programs);
        let protocol_names = program
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect();
        let function_return_types = collect_function_return_types(program, interface_programs);
        let function_type_params = collect_function_type_params(program, interface_programs);
        let function_param_types = collect_function_param_types(program, interface_programs);
        let function_param_defaults = collect_function_param_defaults(program, interface_programs);
        let function_param_effects = collect_function_param_effects(program, interface_programs);
        let retained_params_by_callee =
            collect_function_retained_params(program, interface_programs);

        Self {
            program,
            type_kinds,
            protocol_names,
            native_boundary_callees,
            async_native_boundary_callees,
            native_bindings,
            function_return_types,
            function_type_params,
            function_param_types,
            function_param_defaults,
            function_param_effects,
            retained_params_by_callee,
            param_effects: BTreeMap::new(),
            value_types: BTreeMap::new(),
            managed_bindings: BTreeSet::new(),
            read_view_bindings: BTreeSet::new(),
            current_retained_params: BTreeSet::new(),
            mutated_bindings: BTreeSet::new(),
            drop_field_names: BTreeSet::new(),
            current_return_type: None,
            current_async_executor: None,
            current_task_group_token: None,
            source_map: Vec::new(),
            generic_protocol_bounds: BTreeMap::new(),
        }
    }

    pub(super) fn lower(mut self) -> LoweredRust {
        // Install this program's `#lower_name(...)` pins for the duration of the
        // run so the canonical name-lowering helpers emit the pinned symbols.
        let previous_overrides =
            set_lower_name_overrides(super::collect_lower_name_overrides(self.program));
        let mut out = String::new();
        out.push_str("// Generated by RSScript. Edit the .rss source instead.\n");
        out.push_str(
            "// Runtime hooks are intentionally explicit while Rust lowering is stabilizing.\n",
        );
        // Module-isolated symbols carry a lowercase module prefix (`device__Device`,
        // `helpers__count`), so the camel/snake/upper-case lints don't apply.
        out.push_str(
            "#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_parens)]\n",
        );
        let feature_names = lowered_feature_names(&self.program.features);
        if feature_names.is_empty() {
            out.push_str("// RSScript features: <none>\n");
        } else {
            out.push_str(&format!(
                "// RSScript features: {}\n",
                feature_names.join(", ")
            ));
        }
        out.push('\n');

        for item in &self.program.items {
            if let Item::Type(ty) = item {
                self.lower_type_decl(ty, &mut out);
                out.push('\n');
            }
        }
        for item in &self.program.items {
            if let Item::SumType(sum) = item {
                self.lower_sum_type(sum, &mut out);
                out.push('\n');
            }
        }
        // Lower type aliases
        for item in &self.program.items {
            if let Item::TypeAlias(alias) = item {
                self.lower_type_alias(alias, &mut out);
                out.push('\n');
            }
        }
        // Lower constants
        for item in &self.program.items {
            if let Item::Const(decl) = item {
                self.lower_const_decl(decl, &mut out);
                out.push('\n');
            }
        }
        self.lower_protocol_traits(&mut out);
        self.lower_capability_enums(&mut out);
        for item in &self.program.items {
            match item {
                Item::Type(_)
                | Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
                Item::Function(function) if !function.body.statements.is_empty() => {
                    self.lower_function(function, &mut out);
                    out.push('\n');
                }
                Item::Function(_) => {}
            }
        }
        self.lower_protocol_impls(&mut out);
        self.lower_capability_impls(&mut out);
        while out.ends_with("\n\n") {
            out.pop();
        }

        set_lower_name_overrides(previous_overrides);
        LoweredRust {
            rust_source: out,
            source_map: self.source_map,
        }
    }

    pub(super) fn lower_block(&mut self, block: &Block, out: &mut String, indent: usize) {
        for statement in &block.statements {
            self.lower_stmt(statement, out, indent);
        }
    }

    pub(super) fn lower_expr_block(&mut self, block: &Block, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        let mut out = String::new();
        out.push_str("{\n");
        for (index, statement) in block.statements.iter().enumerate() {
            if index + 1 == block.statements.len()
                && let Stmt::Expr(expr) = statement
            {
                out.push_str(&format!(
                    "{}{}\n",
                    "    ".repeat(indent + 1),
                    self.lower_expr(expr)
                ));
                out.push_str(&format!("{pad}}}"));
                return out;
            }
            self.lower_stmt(statement, &mut out, indent + 1);
        }
        out.push_str(&format!("{pad}}}"));
        out
    }

    pub(super) fn lower_stmt(&mut self, statement: &Stmt, out: &mut String, indent: usize) {
        let pad = "    ".repeat(indent);
        let generated_start = out.len();
        let marker = self.record_source_marker(out, indent, "statement", stmt_span(statement));
        self.record_statement_source_map(statement, &marker.generated);
        match statement {
            Stmt::Let(stmt) => self.lower_let_stmt(stmt, out, &pad),
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    let lowered = self.lower_return_expr(value);
                    out.push_str(&format!("{pad}return {lowered};\n"));
                } else {
                    out.push_str(&format!("{pad}return;\n"));
                }
            }
            Stmt::With(stmt) => {
                let resource = self.lower_expr(&stmt.resource);
                let resource = if is_resource_pool_borrow_expr(&stmt.resource) {
                    format!("rsscript_runtime::unwrap_runtime({resource})")
                } else if is_file_open_expr(&stmt.resource) {
                    format!("{resource}?")
                } else {
                    resource
                };
                out.push_str(&format!("{pad}{{\n"));
                let inner_pad = "    ".repeat(indent + 1);
                out.push_str(&format!(
                    "{inner_pad}let mut {} = {};\n",
                    rust_ident(&stmt.binding),
                    resource
                ));
                self.lower_block(&stmt.body, out, indent + 1);
                self.record_source_marker(out, indent + 1, "resource_drop", &stmt.span);
                out.push_str(&format!("{pad}}}\n"));
            }
            Stmt::If(stmt) => {
                out.push_str(&format!(
                    "{pad}if {} {{\n",
                    self.lower_expr(&stmt.condition)
                ));
                self.lower_block(&stmt.then_body, out, indent + 1);
                if let Some(else_body) = &stmt.else_body {
                    out.push_str(&format!("{pad}}} else {{\n"));
                    self.lower_block(else_body, out, indent + 1);
                }
                out.push_str(&format!("{pad}}}\n"));
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    out.push_str(&format!("{pad}while {} {{\n", self.lower_expr(condition)));
                } else {
                    out.push_str(&format!("{pad}loop {{\n"));
                }
                self.lower_block(&stmt.body, out, indent + 1);
                out.push_str(&format!("{pad}}}\n"));
            }
            Stmt::For(stmt) => self.lower_for_stmt(stmt, out, &pad, indent),
            Stmt::TaskGroup(stmt) => {
                // Structured concurrency scope
                out.push_str(&format!(
                    "{pad}// task_group: structured concurrency scope\n"
                ));
                out.push_str(&format!("{pad}{{\n"));
                self.lower_task_group_block(&stmt.body, out, indent + 1);
                out.push_str(&format!("{pad}}}\n"));
            }
            Stmt::Select(stmt) => {
                out.push_str(&format!(
                    "{pad}// select: wait for the first ready operation\n"
                ));
                self.lower_select_stmt(stmt, out, indent);
            }
            Stmt::Match(stmt) => self.lower_match_stmt(stmt, out, &pad, indent),
            Stmt::LetElse(stmt) => {
                let pattern = lower_match_pattern(&stmt.pattern);
                let value = self.lower_expr(&stmt.value);
                out.push_str(&format!("{pad}let {pattern} = {value} else {{\n"));
                self.lower_block(&stmt.else_body, out, indent + 1);
                out.push_str(&format!("{pad}}};\n"));
                if !stmt.binding_name.is_empty() {
                    let binding_type = self
                        .infer_expr_type(&stmt.value)
                        .and_then(|ty| match_binding_type_ref(&stmt.pattern, Some(&ty)))
                        .map(|(_, ty)| ty);
                    if let Some(binding_type) = binding_type {
                        if self.is_class_type(&binding_type) {
                            self.managed_bindings.insert(stmt.binding_name.clone());
                        } else {
                            self.managed_bindings.remove(&stmt.binding_name);
                        }
                        self.value_types
                            .insert(stmt.binding_name.clone(), binding_type);
                    }
                    self.read_view_bindings.remove(&stmt.binding_name);
                }
            }
            Stmt::Break(_) => out.push_str(&format!("{pad}break;\n")),
            Stmt::Continue(_) => out.push_str(&format!("{pad}continue;\n")),
            Stmt::Assign(stmt) => {
                // An assignment target always needs an owned `T`. When the RHS is a
                // `read`-param ident it lowers to a plain `&T` borrow, so clone it to
                // own the value — the same coercion the `let`-init path applies. (A
                // read-view alias RHS is already cloned inside `lower_expr`.)
                let value = if self.let_init_is_clonable_read_param_ref(&stmt.value) {
                    format!("{}.clone()", self.lower_expr(&stmt.value))
                } else {
                    self.lower_expr(&stmt.value)
                };
                out.push_str(&format!(
                    "{pad}{} = {};\n",
                    self.lower_assignment_target(&stmt.target),
                    value
                ));
            }
            Stmt::Expr(expr) => out.push_str(&format!("{pad}{};\n", self.lower_expr(expr))),
            Stmt::MalformedWith(span)
            | Stmt::MalformedIf(span)
            | Stmt::MalformedLoop(span)
            | Stmt::MalformedFor(span)
            | Stmt::MalformedMatch(span)
            | Stmt::Unknown(span) => unreachable_lowering("statement", span),
        }
        self.widen_generated_span(
            &marker.generated,
            generated_line_count(&out[generated_start..]),
        );
    }

    pub(super) fn lower_let_stmt(&mut self, stmt: &LetStmt, out: &mut String, pad: &str) {
        let mutable = if self.mutated_bindings.contains(&stmt.name)
            || stmt
                .value
                .as_ref()
                .is_some_and(closure_value_mutates_capture)
        {
            "mut "
        } else {
            ""
        };
        if let Some(value) = &stmt.value {
            let lowered = if let Some(expected) = &stmt.type_annotation {
                self.lower_expr_for_expected_type(value, expected)
            } else {
                self.lower_expr(value)
            };
            // `let x = <read-param>` of a managed (non-Copy) type: the
            // read-PARAM lowers to `&T`, so binding it directly makes `x: &T`.
            // That borrow then breaks owned uses of `x` — a later `x = <owned T>`
            // reassignment (for `let mut`), a `return x`, or an alias chain
            // (`let chosen = a; let mut s = chosen`). Clone the borrow into an
            // owned `T` so the local owns its value. Gated tightly by
            // `let_init_is_clonable_read_param_ref`: the initializer must be a
            // `read`-param ident lowering to a plain `&T` (managed, non-Copy).
            let lowered = if self.let_init_is_clonable_read_param_ref(value) {
                format!("{lowered}.clone()")
            } else {
                lowered
            };
            let inferred_ty = self.infer_expr_type(value);
            let annotation = stmt
                .type_annotation
                .as_ref()
                .and_then(|ty| self.lower_let_annotation(ty))
                .map(|ty| format!(": {ty}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "{pad}let {mutable}{}{annotation} = {};\n",
                rust_ident(&stmt.name),
                lowered
            ));
            if let Some(ty) = stmt.type_annotation.clone().or(inferred_ty) {
                self.value_types.insert(stmt.name.clone(), ty);
            }
            if self.expr_lowers_to_managed_handle(value) {
                self.managed_bindings.insert(stmt.name.clone());
            } else {
                self.managed_bindings.remove(&stmt.name);
            }
        } else {
            out.push_str(&format!("{pad}let {mutable}{};\n", rust_ident(&stmt.name)));
            self.managed_bindings.remove(&stmt.name);
        }
    }

    pub(super) fn lower_for_stmt(
        &mut self,
        stmt: &ForStmt,
        out: &mut String,
        pad: &str,
        indent: usize,
    ) {
        let iterable = self.lower_expr(&stmt.iterable);
        let previous_type = self.value_types.get(&stmt.binding).cloned();
        let previous_managed = self.managed_bindings.contains(&stmt.binding);
        let previous_read_view = self.read_view_bindings.contains(&stmt.binding);
        let item_type = self
            .infer_expr_type(&stmt.iterable)
            .as_ref()
            .and_then(|ty| {
                if stmt.is_async {
                    stream_item_type_ref(ty)
                } else {
                    list_element_type_ref(ty)
                }
            });
        if let Some(item_type) = item_type.clone() {
            if self.is_class_type(&item_type) {
                self.managed_bindings.insert(stmt.binding.clone());
            } else {
                self.managed_bindings.remove(&stmt.binding);
            }
            self.value_types.insert(stmt.binding.clone(), item_type);
        }
        if stmt.is_async {
            self.read_view_bindings.remove(&stmt.binding);
            let executor = self
                .current_async_executor
                .clone()
                .unwrap_or_else(|| "__rsscript_async_executor".to_string());
            let prefix = format!(
                "__rsscript_await_for_{}_{}",
                stmt.span.line, stmt.span.column
            );
            out.push_str(&format!("{pad}loop {{\n"));
            out.push_str(&format!(
                "{pad}    let mut {prefix}_pending = rsscript_runtime::stream_next(&{iterable});\n"
            ));
            out.push_str(&format!(
                "{pad}    let {prefix}_next = {executor}.run_pending(&mut {prefix}_pending)?;\n"
            ));
            out.push_str(&format!("{pad}    match {prefix}_next {{\n"));
            out.push_str(&format!(
                "{pad}        Some({}) => {{\n",
                rust_ident(&stmt.binding)
            ));
            self.lower_block(&stmt.body, out, indent + 3);
            out.push_str(&format!("{pad}        }}\n"));
            out.push_str(&format!("{pad}        None => break,\n"));
            out.push_str(&format!("{pad}    }}\n"));
            out.push_str(&format!("{pad}}}\n"));
        } else {
            let iterator = if item_type.as_ref().is_some_and(is_copy_type_ref) {
                self.read_view_bindings.remove(&stmt.binding);
                format!("({iterable}).iter().cloned()")
            } else {
                self.read_view_bindings.insert(stmt.binding.clone());
                format!("({iterable}).iter()")
            };
            out.push_str(&format!(
                "{pad}for {} in {iterator} {{\n",
                rust_ident(&stmt.binding)
            ));
            self.lower_block(&stmt.body, out, indent + 1);
            out.push_str(&format!("{pad}}}\n"));
        }
        match previous_type {
            Some(ty) => {
                self.value_types.insert(stmt.binding.clone(), ty);
            }
            None => {
                self.value_types.remove(&stmt.binding);
            }
        }
        if previous_managed {
            self.managed_bindings.insert(stmt.binding.clone());
        } else {
            self.managed_bindings.remove(&stmt.binding);
        }
        if previous_read_view {
            self.read_view_bindings.insert(stmt.binding.clone());
        } else {
            self.read_view_bindings.remove(&stmt.binding);
        }
    }

    pub(super) fn lower_match_stmt(
        &mut self,
        stmt: &MatchStmt,
        out: &mut String,
        pad: &str,
        indent: usize,
    ) {
        let scrutinee_type = self.infer_expr_type(&stmt.value);
        let mut scrutinee = self.lower_match_scrutinee_expr(&stmt.value, scrutinee_type.as_ref());
        let by_ref = self.match_scrutinee_by_ref(&stmt.value);
        // Native Rust slice patterns match a `[T]`, not a `Vec<T>`; view the
        // list scrutinee as a slice so `[a, b]` / `[first, rest @ ..]` apply.
        if arms_have_list_pattern(&stmt.arms) {
            scrutinee = format!("({scrutinee}).as_slice()");
        }
        out.push_str(&format!("{pad}match {scrutinee} {{\n"));
        for arm in &stmt.arms {
            let pattern =
                self.lower_match_pattern_typed(&arm.pattern, scrutinee_type.as_ref(), by_ref);
            let guard = arm
                .guard
                .as_ref()
                .map(|guard| format!(" if {}", self.lower_expr(guard)))
                .unwrap_or_default();
            out.push_str(&format!(
                "{}{}{} => {{\n",
                "    ".repeat(indent + 1),
                pattern,
                guard
            ));
            let scoped_binding = match_binding_type_ref(&arm.pattern, scrutinee_type.as_ref());
            let previous_value_type = scoped_binding
                .as_ref()
                .and_then(|(binding, _)| self.value_types.get(binding).cloned());
            let previous_managed = scoped_binding
                .as_ref()
                .is_some_and(|(binding, _)| self.managed_bindings.contains(binding));
            if let Some((binding, binding_type)) = &scoped_binding {
                self.value_types
                    .insert(binding.clone(), binding_type.clone());
                if self.is_class_type(binding_type) {
                    self.managed_bindings.insert(binding.clone());
                } else {
                    self.managed_bindings.remove(binding);
                }
            }
            // List arms always bind through a slice, so they need owned
            // rebindings even when the scrutinee itself is owned (not by_ref).
            if by_ref || matches!(arm.pattern, MatchPattern::List { .. }) {
                for (name, rhs) in
                    self.owned_payload_rebindings(&arm.pattern, scrutinee_type.as_ref())
                {
                    out.push_str(&format!(
                        "{}let {name} = {rhs};\n",
                        "    ".repeat(indent + 2)
                    ));
                }
            }
            self.lower_block(&arm.body, out, indent + 2);
            if let Some((binding, _)) = scoped_binding {
                if let Some(previous) = previous_value_type {
                    self.value_types.insert(binding.clone(), previous);
                } else {
                    self.value_types.remove(&binding);
                }
                if previous_managed {
                    self.managed_bindings.insert(binding);
                } else {
                    self.managed_bindings.remove(&binding);
                }
            }
            out.push_str(&format!("{}}},\n", "    ".repeat(indent + 1)));
        }
        out.push_str(&format!("{pad}}}\n"));
    }

    pub(super) fn is_native_boundary_call(&self, callee: &Callee) -> bool {
        let key = native_boundary_callee_key(callee);
        self.native_boundary_callees.contains(&key) || self.native_bindings.contains_key(&key)
    }

    pub(super) fn await_call_lowers_to_pending(&self, callee: &Callee) -> bool {
        let key = native_boundary_callee_key(callee);
        is_async_runtime_intrinsic_callee(callee)
            || (self.async_native_boundary_callees.contains(&key)
                && self.native_bindings.contains_key(&key))
            || self.native_bindings.contains_key(&key)
    }

    /// Whether `name` is a `mut` parameter of a Copy scalar type (Int/Bool/Float/…).
    /// Such a parameter lowers to `&mut T` (call-by-reference write-back), so a bare
    /// ident read/target must dereference it. Non-Copy `mut` params (struct/collection)
    /// keep their `&mut Struct` binding and are never dereferenced here.
    fn is_mut_copy_scalar_param(&self, name: &str) -> bool {
        matches!(self.param_effects.get(name), Some(DataEffect::Mut))
            && self
                .value_types
                .get(name)
                .is_some_and(is_copy_type_ref)
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(name, _) => {
                if self.is_mut_copy_scalar_param(name) {
                    // A `mut` Copy-scalar parameter lowers to `&mut T`; a bare read
                    // dereferences it so the value (and assignment through it) works
                    // against the `&mut i64`/`&mut bool`/… binding. As an assignment
                    // target this yields `(*pos) = …`, writing back to the caller.
                    format!("(*{})", rust_value_ident(name))
                } else if self.drop_field_names.contains(name) {
                    format!("self.{}", rust_ident(name))
                } else if self.read_view_bindings.contains(name)
                    && self
                        .value_types
                        .get(name)
                        .is_some_and(|ty| !is_copy_type_ref(ty))
                {
                    format!("{}.clone()", rust_value_ident(name))
                } else if let Some(sum_name) = self.find_sum_type_for_variant(name) {
                    format!("{}::{}", rust_ident(&sum_name), rust_ident(name))
                } else {
                    lower_builtin_value_ident(name)
                        .map(str::to_string)
                        .unwrap_or_else(|| rust_value_ident(name))
                }
            }
            // Integer literals lower as `i64` (RSScript `Int` is i64); without the
            // suffix Rust infers `i32` for an all-literal sub-expression and can
            // const-overflow at compile time even when the i64 value fits. Float
            // literals already default to `f64` (RSScript `Float`), so leave them.
            Expr::Number(value, _) => {
                if value.contains('.') {
                    value.clone()
                } else {
                    format!("{value}i64")
                }
            }
            Expr::String(value, _) => format!("{:?}.to_string()", decode_string_token(value)),
            Expr::MultilineString(value, _) => format!("{value:?}.to_string()"),
            Expr::ObjectLiteral { .. } => self.lower_json_value(expr),
            Expr::MapLiteral { span, .. } => unreachable_lowering("map literal", span),
            Expr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_owned_expr(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("vec![{items}]")
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let binary_op = *op;
                // Flatten a deep `&&`/`||` chain into a *balanced* tree so the
                // generated Rust nests O(log n) deep instead of O(n): a left-linear
                // chain of hundreds of `&&` overflows rustc's recursive parser/
                // type-checker (the `RUST_MIN_STACK=2g` workaround). `&&`/`||` are
                // associative for both value AND short-circuit/evaluation order, so
                // balanced regrouping is behavior-identical. The chain is collected
                // iteratively (its left spine is the deep part) so this lowerer pass
                // does not itself recurse n-deep.
                if matches!(binary_op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) {
                    let mut rev: Vec<&Expr> = vec![right.as_ref()];
                    let mut node: &Expr = left.as_ref();
                    loop {
                        match node {
                            Expr::Binary {
                                op: inner,
                                left: inner_left,
                                right: inner_right,
                                ..
                            } if *inner == binary_op => {
                                rev.push(inner_right.as_ref());
                                node = inner_left.as_ref();
                            }
                            other => {
                                rev.push(other);
                                break;
                            }
                        }
                    }
                    if rev.len() >= 8 {
                        rev.reverse();
                        let op_str = if binary_op == BinaryOp::LogicalAnd {
                            "&&"
                        } else {
                            "||"
                        };
                        let leaves: Vec<String> = rev
                            .iter()
                            .map(|operand| self.lower_binary_operand(operand, binary_op, false))
                            .collect();
                        return balanced_logical_join(&leaves, op_str);
                    }
                }
                if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                    let method = if *op == BinaryOp::ShiftLeft {
                        "wrapping_shl"
                    } else {
                        "wrapping_shr"
                    };
                    let left = self.lower_expr(left);
                    let right = self.lower_expr(right);
                    return format!("({left} as i64).{method}(({right} as i64).max(0) as u32)");
                }
                let op = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Subtract => "-",
                    BinaryOp::Multiply => "*",
                    BinaryOp::Divide => "/",
                    BinaryOp::Modulo => "%",
                    BinaryOp::BitAnd => "&",
                    BinaryOp::BitOr => "|",
                    BinaryOp::BitXor => "^",
                    BinaryOp::ShiftLeft | BinaryOp::ShiftRight => unreachable!(),
                    BinaryOp::Equal => "==",
                    BinaryOp::NotEqual => "!=",
                    BinaryOp::Less => "<",
                    BinaryOp::LessEqual => "<=",
                    BinaryOp::Greater => ">",
                    BinaryOp::GreaterEqual => ">=",
                    BinaryOp::LogicalAnd => "&&",
                    BinaryOp::LogicalOr => "||",
                };
                if matches!(op, "==" | "!=")
                    && (self.is_string_comparison_operand(left)
                        || self.is_string_comparison_operand(right))
                {
                    return format!(
                        "{} {op} {}",
                        self.lower_string_comparison_operand(left),
                        self.lower_string_comparison_operand(right)
                    );
                }
                if matches!(op, "==" | "!=") {
                    // A `read`-bound enum parameter lowers to `&Op`; comparing it
                    // against a sum *value* (e.g. a bare variant) needs a deref so
                    // both sides are `Op`. Mirrors how field-access enum comparison
                    // already lowers to a value on both sides.
                    let left_ref = self.is_enum_read_ref_operand(left);
                    let right_ref = self.is_enum_read_ref_operand(right);
                    if left_ref && !right_ref && self.is_enum_value_operand(right) {
                        return format!(
                            "*{} {op} {}",
                            self.lower_binary_operand(left, binary_op, false),
                            self.lower_binary_operand(right, binary_op, true)
                        );
                    }
                    if right_ref && !left_ref && self.is_enum_value_operand(left) {
                        return format!(
                            "{} {op} *{}",
                            self.lower_binary_operand(left, binary_op, false),
                            self.lower_binary_operand(right, binary_op, true)
                        );
                    }
                }
                format!(
                    "{} {op} {}",
                    self.lower_binary_operand(left, binary_op, false),
                    self.lower_binary_operand(right, binary_op, true)
                )
            }
            Expr::Field { base, name, span } => {
                if self
                    .infer_expr_type(base)
                    .is_some_and(|ty| self.is_class_type(&ty))
                    || self.expr_lowers_to_managed_non_class_handle(base)
                {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_read_at({})).{}.clone()",
                        self.lower_expr(base),
                        lower_source_span(span),
                        rust_ident(name)
                    )
                } else if self.expr_is_read_view(base) {
                    let lowered = format!(
                        "{}.{}",
                        self.lower_read_view_base_expr(base),
                        rust_ident(name)
                    );
                    if self
                        .infer_expr_type(expr)
                        .is_some_and(|ty| !is_copy_type_ref(&ty))
                    {
                        format!("{lowered}.clone()")
                    } else {
                        lowered
                    }
                } else {
                    format!("{}.{}", self.lower_expr(base), rust_ident(name))
                }
            }
            Expr::Index { base, index, .. } => {
                // RSScript indices are `Int` (i64), but Rust slice indices are
                // `usize`; cast explicitly (parenthesized so `as` binds to the
                // whole index expression, not just its trailing literal).
                format!(
                    "{}[({}) as usize]",
                    self.lower_expr(base),
                    self.lower_expr(index)
                )
            }
            Expr::Call { callee, args, span } => {
                // Thin router: each `lower_call_*` helper recognizes one family of
                // call shapes and returns `Some(rust)` if it applies; the final
                // `lower_call_dispatch` handles the generic/fallthrough call forms.
                if let Some(lowered) = self.lower_call_json_codec(callee, args, span) {
                    return lowered;
                }
                if let Some(lowered) = self.lower_call_task_cancellation_token(callee) {
                    return lowered;
                }
                if let Some(lowered) = self.lower_call_named_constructor(callee, args, span) {
                    return lowered;
                }
                if let Some(lowered) = self.lower_call_receiver(callee, args) {
                    return lowered;
                }
                self.lower_call_dispatch(callee, args, span)
            }
            Expr::Effect {
                effect,
                value,
                span,
            } => match effect {
                DataEffect::Read => {
                    if self.expr_is_read_view(value) {
                        self.lower_read_view_expr(value)
                    } else if self.expr_lowers_to_managed_non_class_handle(value) {
                        format!(
                            "&*rsscript_runtime::unwrap_runtime({}.try_read_at({}))",
                            self.lower_expr(value),
                            lower_source_span(span)
                        )
                    } else {
                        format!("&({})", self.lower_expr(value))
                    }
                }
                DataEffect::Mut => {
                    if let Expr::Ident(name, _) = &**value
                        && self.param_effects.get(name) == Some(&DataEffect::Mut)
                    {
                        rust_value_ident(name)
                    } else if self
                        .infer_expr_type(value)
                        .is_some_and(|ty| self.is_class_type(&ty))
                    {
                        format!("&{}", self.lower_expr(value))
                    } else if self.expr_lowers_to_managed_non_class_handle(value) {
                        format!(
                            "&mut *rsscript_runtime::unwrap_runtime({}.try_write_at({}))",
                            self.lower_expr(value),
                            lower_source_span(span)
                        )
                    } else {
                        format!("&mut {}", self.lower_expr(value))
                    }
                }
                DataEffect::Take => self.lower_expr(value),
            },
            Expr::Manage { value, span } => {
                format!(
                    "rsscript_runtime::manage_at({}, {})",
                    self.lower_expr(value),
                    lower_source_span(span)
                )
            }
            Expr::Spawn { span, .. } => unreachable_lowering("spawn expression", span),
            Expr::Await { value, .. } => self.lower_await_expr(value),
            Expr::Try { value, .. } => format!("{}?", self.lower_expr(value)),
            Expr::Closure { params, body, .. } => {
                let lowered_params = params
                    .iter()
                    .map(|param| rust_ident(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                let previous_return_type = self.current_return_type.take();
                if let [Stmt::Expr(value)] = body.statements.as_slice() {
                    let lowered = format!("|{lowered_params}| {}", self.lower_expr(value));
                    self.current_return_type = previous_return_type;
                    return lowered;
                }
                let mut out = String::new();
                out.push_str(&format!("|{lowered_params}| {{\n"));
                self.lower_block(body, &mut out, 1);
                out.push('}');
                self.current_return_type = previous_return_type;
                out
            }
            Expr::Match { value, arms, .. } => {
                let scrutinee_type = self.infer_expr_type(value);
                let mut scrutinee = self.lower_match_scrutinee_expr(value, scrutinee_type.as_ref());
                let by_ref = self.match_scrutinee_by_ref(value);
                if arms_have_list_pattern(arms) {
                    scrutinee = format!("({scrutinee}).as_slice()");
                }
                let mut out = format!("match {scrutinee} {{\n");
                for arm in arms {
                    let pattern = self.lower_match_pattern_typed(
                        &arm.pattern,
                        scrutinee_type.as_ref(),
                        by_ref,
                    );
                    let guard = arm
                        .guard
                        .as_ref()
                        .map(|guard| format!(" if {}", self.lower_expr(guard)))
                        .unwrap_or_default();
                    let mut body = self.lower_expr_block(&arm.body, 1);
                    if by_ref || matches!(arm.pattern, MatchPattern::List { .. }) {
                        let binds =
                            self.owned_payload_rebindings(&arm.pattern, scrutinee_type.as_ref());
                        if !binds.is_empty() {
                            let rebinds = binds
                                .iter()
                                .map(|(n, rhs)| format!("let {n} = {rhs};"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            body = body.replacen('{', &format!("{{ {rebinds}"), 1);
                        }
                    }
                    out.push_str(&format!("    {pattern}{guard} => {body}"));
                    out.push_str(",\n");
                }
                out.push('}');
                out
            }
            Expr::Unknown(span) => unreachable_lowering("expression", span),
        }
    }

    /// `json_decode`/`json_encode` codec calls.
    pub(super) fn lower_call_json_codec(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<String> {
        if is_json_decode_callee(callee) {
            return Some(self.lower_json_decode_call(callee, args, span));
        }
        if is_json_encode_callee(callee)
            && let Some(value_arg) = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some("value") || arg.name.is_none())
        {
            return Some(self.lower_json_value(&value_arg.value));
        }
        None
    }

    /// `Task.cancellation_token()` resolves to the enclosing task_group's token,
    /// or a never-cancelled token outside one.
    pub(super) fn lower_call_task_cancellation_token(&mut self, callee: &Callee) -> Option<String> {
        if let Callee::Qualified { namespace, name } = callee
            && type_root_name(namespace) == "Task"
            && type_root_name(name) == "cancellation_token"
        {
            return Some(match &self.current_task_group_token {
                Some(guard) => format!("{guard}.token()"),
                None => "rsscript_runtime::cancellation_never()".to_string(),
            });
        }
        None
    }

    /// `Callee::Name` constructor forms: user struct/class constructors, user
    /// sum-type payload-variant construction, Rust enum constructors, and
    /// runtime struct constructors. Returns `None` if the callee is not a name
    /// or names no known constructor (so the call falls through to dispatch).
    pub(super) fn lower_call_named_constructor(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<String> {
        let Callee::Name(name) = callee else {
            return None;
        };
        // A turbofish constructor callee carries its type arguments in the
        // raw name (e.g. `Pair<Int>`), but `type_kinds` and the per-type
        // field/handle lookups are keyed by the bare root (`Pair`). Root the
        // name so the named-field constructor path is taken (and the struct
        // literal is emitted as `Pair { .. }`, letting Rust infer the args)
        // instead of falling through to a positional tuple-struct call.
        let ctor_name = type_root_name(name);
        if let Some(type_kind) = self.type_kinds.get(ctor_name).copied() {
            let mut fields = Vec::new();
            for arg in args {
                let field_name = self.constructor_field_arg_name(ctor_name, arg);
                let field = field_name
                    .as_deref()
                    .map(rust_ident)
                    .unwrap_or_else(|| "/* unnamed */".to_string());
                let is_weak_field = arg
                    .name
                    .as_deref()
                    .or(field_name.as_deref())
                    .is_some_and(|field_name| self.is_weak_field(ctor_name, field_name));
                if is_weak_field {
                    fields.push(format!(
                        "{field}: {}",
                        self.lower_explicit_weak_field_value(&arg.value)
                    ));
                } else if field_name
                    .as_deref()
                    .is_some_and(|field_name| self.is_runtime_handle_field(ctor_name, field_name))
                {
                    let field_name = field_name.unwrap_or_default();
                    fields.push(format!(
                        "{field}: {}",
                        self.lower_runtime_handle_field_value(
                            ctor_name,
                            &field_name,
                            &arg.value,
                            &arg.span
                        )
                    ));
                } else {
                    let value = field_name
                        .as_deref()
                        .and_then(|field_name| self.field_type(ctor_name, field_name))
                        .map(|expected| self.lower_expr_for_expected_type(&arg.value, &expected))
                        .unwrap_or_else(|| self.lower_owned_expr(&arg.value));
                    fields.push(format!("{field}: {value}"));
                }
            }
            // Fill omitted fields that declare a default value
            // (`name: T = expr`) so the Rust struct literal is complete.
            let provided: std::collections::HashSet<String> = args
                .iter()
                .filter_map(|arg| self.constructor_field_arg_name(ctor_name, arg))
                .collect();
            let defaulted: Vec<(String, Expr)> = self
                .program
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Type(decl) if type_root_name(&decl.name) == ctor_name => Some(
                        decl.fields
                            .iter()
                            .filter_map(|f| f.default.clone().map(|d| (f.name.clone(), d)))
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            for (field_name, default) in defaulted {
                if provided.contains(&field_name) {
                    continue;
                }
                let value = self
                    .field_type(ctor_name, &field_name)
                    .map(|expected| self.lower_expr_for_expected_type(&default, &expected))
                    .unwrap_or_else(|| self.lower_owned_expr(&default));
                fields.push(format!("{}: {value}", rust_ident(&field_name)));
            }
            let fields = fields.join(", ");
            let constructed = format!("{} {{ {fields} }}", rust_ident(ctor_name));
            if type_kind == TypeKind::Class {
                return Some(format!(
                    "rsscript_runtime::manage_at({constructed}, {})",
                    lower_source_span(span)
                ));
            }
            return Some(constructed);
        }

        // User sum-type payload-variant construction: emit the qualified, struct-style
        // form `Enum::Variant { field: value, ... }` to match the lowered enum (whose
        // payload variants use named fields). Nullary variants emit `Enum::Variant`.
        if let Some(sum_name) = self.find_sum_type_for_variant(ctor_name) {
            // declared (name, type) of each field of this variant
            let decl_fields: Vec<(String, TypeRef)> = self
                .program
                .items
                .iter()
                .find_map(|item| {
                    if let Item::SumType(sum) = item {
                        sum.variants.iter().find(|v| v.name == ctor_name).map(|v| {
                            v.fields
                                .iter()
                                .map(|f| (f.name.clone(), f.ty.clone()))
                                .collect()
                        })
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            if decl_fields.is_empty() {
                return Some(format!(
                    "{}::{}",
                    rust_ident(&sum_name),
                    rust_ident(ctor_name)
                ));
            }
            let mut fields = Vec::new();
            for (index, arg) in args.iter().enumerate() {
                // Resolve the target field by name if given, else positionally. Arity and
                // field names are validated in the checker; here we stay bounds-safe and
                // lower each value against its declared field type (like struct ctors).
                let field = match &arg.name {
                    Some(name) => decl_fields.iter().find(|(fname, _)| fname == name),
                    None => decl_fields.get(index),
                };
                let Some((field_name, field_ty)) = field else {
                    continue;
                };
                let value = self.lower_expr_for_expected_type(&arg.value, field_ty);
                fields.push(format!("{}: {value}", rust_ident(field_name)));
            }
            return Some(format!(
                "{}::{} {{ {} }}",
                rust_ident(&sum_name),
                rust_ident(ctor_name),
                fields.join(", ")
            ));
        }

        if is_rust_enum_constructor(name) {
            let args = args
                .iter()
                .map(|arg| self.lower_expr(&arg.value))
                .collect::<Vec<_>>()
                .join(", ");
            return Some(format!("{}({args})", rust_ident(name)));
        }

        if let Some((target, fields)) = runtime_struct_constructor(name) {
            let mut lowered_fields = Vec::new();
            for (index, arg) in args.iter().enumerate() {
                let Some(field) = arg.name.as_deref().or_else(|| fields.get(index).copied()) else {
                    continue;
                };
                lowered_fields.push(format!(
                    "{}: {}",
                    rust_ident(field),
                    self.lower_owned_expr(&arg.value)
                ));
            }
            return Some(format!("{target} {{ {} }}", lowered_fields.join(", ")));
        }

        None
    }

    /// Receiver-call shorthand (`receiver.method(..)`): resolve the receiver's
    /// type, pick the protocol/facade/native namespace, apply the receiver's
    /// borrow/effect, and emit the qualified call with the receiver as the first
    /// argument. Returns `None` if the callee is not a receiver call.
    pub(super) fn lower_call_receiver(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
    ) -> Option<String> {
        if let Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } = callee
        {
            let receiver_type = self.infer_expr_type(receiver);
            let receiver_type_name = receiver_type
                .as_ref()
                .map(type_ref_display_name)
                .unwrap_or_else(|| "Unknown".to_string());
            let receiver_type_root = type_root_name(&receiver_type_name).to_string();
            if method == "clone"
                && (self.type_derives_clone(&receiver_type_root)
                    || Self::is_builtin_clone_value(&receiver_type_root))
            {
                // Explicit `.clone()` -> Rust's `Clone`: a user struct/sum that derives
                // `Clone`, or a builtin value type that lowers to a `Clone` Rust type but has
                // no dedicated clone intrinsic (`List`/`Map`/`Set`/…). Without the builtin
                // case these fell through to a dangling `List_clone`-style call (the checker
                // accepts `.clone()` via the `Clone` protocol, so it never errored at the
                // front end — E0425 at the Rust backend). `.clone()` borrows its receiver, so
                // a place behind a `&` (read param / field of one) works without moving. Types
                // that don't derive Clone are rejected earlier (RS0206). `String`/`Json` keep
                // their own clone intrinsics and are handled below.
                return Some(format!("{}.clone()", self.lower_expr(receiver)));
            }
            let receiver_rust_type = receiver_type
                .as_ref()
                .map(|ty| self.lower_type_ref(ty, ManagedPosition::Bare))
                .unwrap_or_else(|| rust_ident(&receiver_type_name));
            // Resolve namespace: check generic protocol bounds first,
            // then concrete protocol impls, then fall back to the type
            // name itself. This mirrors HIR receiver-call resolution for
            // the non-ambiguous cases that are allowed to reach lowering.
            let namespace = self
                .generic_protocol_bounds
                .get(&receiver_type_name)
                .cloned()
                .or_else(|| capability_protocol_name(&receiver_type_name).map(str::to_string))
                .or_else(|| self.protocol_impl_namespace(&receiver_type_root, method))
                .or_else(|| {
                    receiver_facade_namespace(&receiver_type_root, method).map(str::to_string)
                })
                .unwrap_or(receiver_type_root);
            let is_protocol = self.protocol_names.contains(&namespace);
            let lowered_receiver = match (*effect).unwrap_or(DataEffect::Read) {
                DataEffect::Mut
                    if let Expr::Ident(receiver_name, _) = receiver.as_ref()
                        && self.param_effects.get(receiver_name) == Some(&DataEffect::Mut) =>
                {
                    rust_ident(receiver_name)
                }
                DataEffect::Mut
                    if matches!(
                        receiver.as_ref(),
                        Expr::Ident(..) | Expr::Field { .. } | Expr::Index { .. }
                    ) =>
                {
                    format!("&mut {}", self.lower_assignment_target(receiver))
                }
                DataEffect::Mut => format!("&mut {}", self.lower_expr(receiver)),
                DataEffect::Read if receiver_type.as_ref().is_some_and(is_copy_type_ref) => {
                    self.lower_expr(receiver)
                }
                DataEffect::Read
                    if receiver_type.as_ref().is_some_and(|ty| {
                        (ty.name == "List"
                            && matches!(receiver.as_ref(), Expr::ArrayLiteral { .. }))
                            || (ty.name == "Map"
                                && matches!(receiver.as_ref(), Expr::MapLiteral { .. }))
                    }) =>
                {
                    let receiver_type = receiver_type.as_ref().expect("checked above");
                    format!(
                        "&{}",
                        self.lower_expr_for_expected_type(receiver, receiver_type)
                    )
                }
                DataEffect::Read => format!("&{}", self.lower_expr(receiver)),
                DataEffect::Take => self.lower_expr(receiver),
            };
            // Runtime intrinsics / native fns take their args by reference
            // (even `Copy` ones), so for those callees honor the parameter
            // effect for named args too. Protocol/user receiver calls are
            // not native targets and keep their by-value `Copy` handling.
            let native_target_callee = Callee::Qualified {
                namespace: namespace.clone(),
                name: method.to_string(),
            };
            let is_native_target = runtime_intrinsic_target(&native_target_callee).is_some()
                || self
                    .native_bindings
                    .contains_key(&native_boundary_function_key(&format!(
                        "{namespace}.{method}"
                    )));
            let lowered_args = args
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    if let Some(expected) = self.receiver_call_expected_arg_type(
                        &namespace,
                        method,
                        receiver_type.as_ref(),
                        arg.name.as_deref(),
                        index,
                    ) {
                        // Only `Copy` named args to native targets need the
                        // effect-based `&` (they'd otherwise pass by value to a
                        // by-reference intrinsic); non-Copy named args keep their
                        // existing lowering (which handles String/Json cleanly).
                        if (arg.name.is_none() || (is_native_target && is_copy_type_ref(&expected)))
                            && let Some(effect) =
                                self.receiver_call_expected_arg_effect(&namespace, method, index)
                        {
                            let lowered = self.lower_receiver_positional_arg(&arg.value, &expected);
                            return match effect {
                                DataEffect::Read => format!("&{lowered}"),
                                DataEffect::Mut => format!("&mut {lowered}"),
                                DataEffect::Take => lowered,
                            };
                        }
                        if arg.name.is_none() {
                            if expected.name == "Fn" {
                                return self.lower_expr_for_expected_type(&arg.value, &expected);
                            }
                            let lowered = self.lower_receiver_positional_arg(&arg.value, &expected);
                            if is_copy_type_ref(&expected) {
                                return lowered;
                            }
                            return format!("&{lowered}");
                        }
                        return self.lower_call_arg_for_expected_type(&arg.value, &expected);
                    }
                    self.lower_expr(&arg.value)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let all_args = if lowered_args.is_empty() {
                lowered_receiver
            } else {
                format!("{lowered_receiver}, {lowered_args}")
            };
            let qualified_key = native_boundary_function_key(&format!("{namespace}.{method}"));
            if let Some(native_target) = self.native_bindings.get(&qualified_key).cloned() {
                return Some(format!("{native_target}({all_args})"));
            }
            let callee_str = if is_protocol {
                format!(
                    "<{} as {}>::{}",
                    receiver_rust_type,
                    rust_ident(&namespace),
                    rust_ident(method)
                )
            } else {
                lower_callee(&Callee::Qualified {
                    namespace: namespace.clone(),
                    name: method.clone(),
                })
            };
            return Some(format!("{callee_str}({all_args})"));
        }

        None
    }

    /// Generic / fallthrough call dispatch: string-concat and weak intrinsics,
    /// native-bound free functions, resource-pool constructors and borrows,
    /// capability-from-protocol, protocol callees, and the default
    /// `callee(args...)` form (including trailing defaulted-parameter fill-in).
    pub(super) fn lower_call_dispatch(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> String {
        if is_string_concat_callee(callee) {
            return lower_string_concat_call(self, args);
        }
        if is_weak_from_callee(callee) {
            return lower_weak_from_call(self, args);
        }
        if is_weak_upgrade_callee(callee) {
            return lower_weak_upgrade_call(self, args);
        }
        if let Some(native_target) = self
            .native_bindings
            .get(&native_boundary_callee_key(callee))
            .cloned()
        {
            let args = args
                .iter()
                .map(|arg| self.lower_expr(&arg.value))
                .collect::<Vec<_>>()
                .join(", ");
            return format!("{native_target}({args})");
        }
        if is_resource_pool_new_callee(callee) {
            return lower_resource_pool_new_call(self, args, span);
        }
        if is_resource_pool_try_new_callee(callee) {
            return lower_resource_pool_try_new_call(self, args, span);
        }
        if is_resource_pool_lazy_callee(callee) {
            return lower_resource_pool_lazy_call(self, args, span);
        }
        if is_resource_pool_try_lazy_callee(callee) {
            return lower_resource_pool_try_lazy_call(self, args, span);
        }
        if let Some(protocol) = capability_from_protocol(callee) {
            return self.lower_capability_from_call(protocol, args);
        }
        if self.is_protocol_callee(callee) {
            let lowered_callee = lower_protocol_callee(callee);
            let args = args
                .iter()
                .enumerate()
                .map(|(index, arg)| self.lower_call_arg_for_callee(callee, arg, index))
                .collect::<Vec<_>>()
                .join(", ");
            return format!("{lowered_callee}({args})");
        }
        let is_resource_pool_borrow = is_resource_pool_borrow_callee(callee);
        let lowered_callee = if is_resource_pool_borrow {
            "rsscript_runtime::ResourcePool::borrow_at".to_string()
        } else if is_resource_pool_try_borrow_callee(callee) {
            "rsscript_runtime::ResourcePool::try_borrow".to_string()
        } else {
            lower_callee(callee)
        };
        let provided = args.len();
        let mut args = args
            .iter()
            .enumerate()
            .map(|(index, arg)| self.lower_call_arg_for_callee(callee, arg, index))
            .collect::<Vec<_>>();
        // Fill omitted trailing parameters that declare a default value
        // (Rust has no default params, so each call site supplies them).
        // Calls lower positionally, so omitted args are always the trailing
        // defaulted ones.
        if let Some(name) = callee_source_name(callee)
            && let Some(defaults) = self.function_param_defaults.get(&name).cloned()
            && defaults.len() > provided
        {
            let param_types = self.function_param_types.get(&name).cloned();
            let param_effects = self
                .function_param_effects
                .get(&native_boundary_callee_key(callee))
                .cloned();
            for (index, default) in defaults.iter().enumerate().skip(provided) {
                if let Some(default) = default {
                    let effect = param_effects
                        .as_ref()
                        .and_then(|params| params.get(index))
                        .and_then(|(_, effect)| *effect);
                    let lowered = if let Some(effect) = effect {
                        // A non-Copy default is materialized at the call and
                        // passed under the parameter's declared effect;
                        // route it through the normal argument path so the
                        // borrow/managed-handle ABI matches the signature.
                        let synthetic = CallArg {
                            name: param_types
                                .as_ref()
                                .and_then(|params| params.get(index))
                                .map(|(param_name, _)| param_name.clone()),
                            value: Expr::Effect {
                                effect,
                                value: Box::new(default.clone()),
                                span: span.clone(),
                            },
                            malformed: false,
                            span: span.clone(),
                        };
                        self.lower_call_arg_for_callee(callee, &synthetic, index)
                    } else {
                        match param_types.as_ref().and_then(|params| params.get(index)) {
                            Some((_, ty)) => self.lower_expr_for_expected_type(default, ty),
                            None => self.lower_owned_expr(default),
                        }
                    };
                    args.push(lowered);
                }
            }
        }
        if is_resource_pool_borrow {
            args.push(lower_source_span(span));
        }
        let args = args.join(", ");
        format!("{lowered_callee}({args})")
    }

    pub(super) fn lower_binary_operand(
        &mut self,
        expr: &Expr,
        parent: BinaryOp,
        is_right: bool,
    ) -> String {
        let lowered = self.lower_expr(expr);
        let Expr::Binary { op: child, .. } = expr else {
            return lowered;
        };
        let parent_precedence = rust_binary_precedence(parent);
        let child_precedence = rust_binary_precedence(*child);
        let chained_comparison =
            rust_binary_is_comparison(parent) && rust_binary_is_comparison(*child);
        if child_precedence < parent_precedence
            || (is_right && child_precedence == parent_precedence)
            || chained_comparison
        {
            format!("({lowered})")
        } else {
            lowered
        }
    }

    pub(super) fn lower_owned_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                span,
            }
            | Expr::Effect {
                effect: DataEffect::Mut,
                value,
                span,
            } => {
                if self.expr_lowers_to_managed_non_class_handle(value) {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_read_at({})).clone()",
                        self.lower_expr(value),
                        lower_source_span(span)
                    )
                } else {
                    format!("{}.clone()", self.lower_expr(value))
                }
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_expr(value),
            _ => self.lower_expr(expr),
        }
    }

    pub(super) fn lower_expr_for_expected_type(
        &mut self,
        expr: &Expr,
        expected: &TypeRef,
    ) -> String {
        if expected.name == "Fn"
            && let Expr::Closure { params, body, .. } = expr
        {
            // Every caller of this entry point places the closure into a
            // STORABLE slot (struct/sum field init, return value, collection or
            // Option/Result payload). A storable `owned Fn` slot lowers to
            // `Rc<dyn Fn>`, so the closure literal is wrapped in `Rc::new`. The
            // sole non-storable (direct parameter) case is handled separately in
            // `lower_call_arg_for_expected_type`.
            return self
                .lower_closure_for_expected_fn(params, body, expected, /* storable */ true);
        }
        if let Expr::Call {
            callee: Callee::Name(name),
            args,
            ..
        } = expr
        {
            if expected.name == "Result" && expected.args.len() == 2 {
                match (name.as_str(), args.as_slice()) {
                    ("Ok", [arg]) => {
                        let payload =
                            self.lower_owned_expr_for_expected_type(&arg.value, &expected.args[0]);
                        return format!("Ok({payload})");
                    }
                    ("Err", [arg]) => {
                        let payload =
                            self.lower_owned_expr_for_expected_type(&arg.value, &expected.args[1]);
                        return format!("Err({payload})");
                    }
                    ("Ok", []) if expected.args[0].name == "Unit" => return "Ok(())".to_string(),
                    ("Err", []) if expected.args[1].name == "Unit" => return "Err(())".to_string(),
                    _ => {}
                }
            }
            if expected.name == "Option" && expected.args.len() == 1 {
                match (name.as_str(), args.as_slice()) {
                    ("Some", [arg]) => {
                        let payload =
                            self.lower_owned_expr_for_expected_type(&arg.value, &expected.args[0]);
                        return format!("Some({payload})");
                    }
                    ("Some", []) if expected.args[0].name == "Unit" => {
                        return "Some(())".to_string();
                    }
                    ("None", []) => return "None".to_string(),
                    _ => {}
                }
            }
        }
        if expected.name == "JsonValue" && expr_is_json_literal(expr) {
            let lowered = self.lower_json_value(expr);
            return format!("rsscript_runtime::json_value(&{lowered})");
        }
        if expected.name == "Map" && matches!(expr, Expr::MapLiteral { .. }) {
            return self.lower_map_literal(expr, expected);
        }
        if expected.name == "List" && matches!(expr, Expr::ArrayLiteral { .. }) {
            return self.lower_list_literal(expr, expected);
        }
        if expected.name == "String"
            && expected.args.is_empty()
            && let Expr::Ident(name, _) = expr
            && self.param_effects.get(name) == Some(&DataEffect::Read)
        {
            return format!("{}.clone()", rust_value_ident(name));
        }
        if let Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } = expr
        {
            let lowered = self.lower_expr(value);
            if is_copy_type_ref(expected) {
                return lowered;
            }
            return format!("{lowered}.clone()");
        }
        // Integer literal in a typed slot (e.g. a sized `Int32` param/field):
        // emit the suffix matching the expected type so it lowers as the right
        // Rust integer rather than `lower_expr`'s default `i64`.
        if let Expr::Number(value, _) = expr
            && !value.contains('.')
            && let Some(suffix) = rust_int_literal_suffix(&expected.name)
        {
            return format!("{value}{suffix}");
        }
        self.lower_expr(expr)
    }

    pub(super) fn lower_closure_for_expected_fn(
        &mut self,
        params: &[String],
        body: &Block,
        expected: &TypeRef,
        storable: bool,
    ) -> String {
        // A storable `owned Fn` slot is `Rc<dyn Fn(..)>`; wrap the closure value
        // in `Rc::new(..)` so it matches that type and stays `Clone`/shareable.
        // The closure itself always `move`s its captures (owned/Copy only, per
        // the checker's capture-soundness rule), so it can outlive the builder.
        let wrap_rc = storable && expected.is_owned;
        let inner = self.lower_closure_for_expected_fn_inner(params, body, expected);
        if wrap_rc {
            format!("std::rc::Rc::new({inner})")
        } else {
            inner
        }
    }

    pub(super) fn lower_closure_for_expected_fn_inner(
        &mut self,
        params: &[String],
        body: &Block,
        expected: &TypeRef,
    ) -> String {
        let previous_value_types = self.value_types.clone();
        let previous_param_effects = self.param_effects.clone();
        for (param, ty) in params.iter().zip(expected.fn_params.iter()) {
            self.value_types.insert(param.clone(), ty.clone());
        }
        // Register each closure parameter's data effect so the body lowers
        // exactly like a regular function with the same parameter effects: a
        // `read T` parameter is a borrowed `&T` (reads `.clone()` out of it where
        // needed), a `mut T` parameter is an exclusive `&mut T` whose field
        // assignments propagate to the caller, and a `take`/defaulted parameter is
        // owned by value. This is what lets a stored `mut Ctx` rule mutate `ctx`.
        for (index, param) in params.iter().enumerate() {
            if let Some(effect) = expected.fn_param_effects.get(index).copied().flatten() {
                self.param_effects.insert(param.clone(), effect);
            }
        }
        // The closure's parameter list must match the stored
        // `Rc<dyn Fn(&UOp, &mut Ctx) -> ..>` signature: annotate each parameter
        // with the same ref it would carry as a regular function parameter
        // (`read T` -> `&T`, `mut T` -> `&mut T`, owned otherwise), mirroring
        // `lower_param`'s effect-to-ref mapping.
        let lowered_params = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let name = rust_ident(param);
                let Some(ty) = expected
                    .fn_params
                    .get(index)
                    .filter(|ty| self.type_ref_is_concrete_for_annotation(ty))
                else {
                    return name;
                };
                let bare = self.lower_type_ref(ty, ManagedPosition::Param);
                let effect = expected.fn_param_effects.get(index).copied().flatten();
                // Match the stored `Rc<dyn Fn(..)>` parameter ABI exactly (see
                // `lower_type_ref` for `Fn`): `read T` -> `&T`, `mut T` -> `&mut T`,
                // owned otherwise. Kept uniform (no by-value-`Copy` shortcut) so
                // the closure literal, its stored type, and every call site agree.
                let rust_ty = match effect {
                    Some(DataEffect::Read) => format!("&{bare}"),
                    Some(DataEffect::Mut) => format!("&mut {bare}"),
                    Some(DataEffect::Take) | None => bare,
                };
                format!("{name}: {rust_ty}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let previous_return_type = self.current_return_type.take();
        let closure_prefix = if expected.is_owned { "move " } else { "" };
        if let [Stmt::Expr(value)] = body.statements.as_slice() {
            let lowered = format!(
                "{closure_prefix}|{lowered_params}| {}",
                self.lower_expr(value)
            );
            self.current_return_type = previous_return_type;
            self.value_types = previous_value_types;
            self.param_effects = previous_param_effects;
            return lowered;
        }
        let mut out = String::new();
        out.push_str(&format!("{closure_prefix}|{lowered_params}| {{\n"));
        self.lower_block(body, &mut out, 1);
        out.push('}');
        self.current_return_type = previous_return_type;
        self.value_types = previous_value_types;
        self.param_effects = previous_param_effects;
        out
    }

    pub(super) fn lower_json_decode_call(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> String {
        let Some(arg) = args
            .iter()
            .find(|arg| {
                arg.name
                    .as_deref()
                    .is_some_and(|name| name == "value" || name == "text")
                    || arg.name.is_none()
            })
            .or_else(|| args.first())
        else {
            unreachable_lowering("Json.decode call argument", span)
        };
        let type_suffix = json_decode_type_arg(callee)
            .map(|name| {
                let ty = type_ref_from_display(name, span);
                format!("::<{}>", self.lower_type_ref(&ty, ManagedPosition::Nested))
            })
            .unwrap_or_default();
        if is_json_decode_text_callee(callee) {
            let text = self.lower_decode_read_arg(&arg.value);
            format!("rsscript_runtime::json_decode_text{type_suffix}(&{text})")
        } else {
            let json_ty = simple_type_ref("JsonValue", span);
            let value = match &arg.value {
                Expr::Effect {
                    effect: DataEffect::Read,
                    value,
                    ..
                } => self.lower_expr_for_expected_type(value, &json_ty),
                _ => self.lower_expr_for_expected_type(&arg.value, &json_ty),
            };
            format!("rsscript_runtime::json_decode_value{type_suffix}(&{value})")
        }
    }

    pub(super) fn lower_decode_read_arg(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                ..
            } => self.lower_expr(value),
            _ => self.lower_expr(expr),
        }
    }

    pub(super) fn lower_assignment_target(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Field { base, name, span } => {
                let base_is_managed_class = self
                    .infer_expr_type(base)
                    .is_some_and(|ty| self.is_class_type(&ty));
                if base_is_managed_class || self.expr_lowers_to_managed_non_class_handle(base) {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_write_at({})).{}",
                        self.lower_expr(base),
                        lower_source_span(span),
                        rust_ident(name)
                    )
                } else {
                    format!(
                        "{}.{}",
                        self.lower_assignment_target(base),
                        rust_ident(name)
                    )
                }
            }
            Expr::Index { base, index, .. } => {
                format!(
                    "{}[{} as usize]",
                    self.lower_assignment_target(base),
                    self.lower_expr(index)
                )
            }
            Expr::Ident(..) => self.lower_expr(expr),
            _ => self.lower_expr(expr),
        }
    }

    pub(super) fn lower_call_arg_for_callee(
        &mut self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> String {
        // Calling a first-class closure value (`let f = r.fxn; f(read u, mut ctx)`):
        // the callee is `Rc<dyn Fn(P0, P1, ..) -> R>` whose parameters carry the
        // stored `Fn` type's per-parameter data effects. Pass each argument to
        // match that effect's Rust ABI: `read T` -> `&T` (by value for `Copy`),
        // `mut T` -> `&mut T` (the closure may mutate it; the borrow is exclusive
        // for the call), and a `take`/defaulted parameter by value (a `read` of a
        // non-`Copy` value becomes an owned `.clone()`). This mirrors `lower_param`
        // and the stored `Rc<dyn Fn(&UOp, &mut Ctx)>` signature.
        if self.callee_is_closure_value(callee) {
            let effect = self.closure_value_param_effect(callee, index);
            let inner = match &arg.value {
                Expr::Effect { value, .. } => value.as_ref(),
                other => other,
            };
            return match effect {
                Some(DataEffect::Read) => format!("&{}", self.lower_expr(inner)),
                Some(DataEffect::Mut) => {
                    // When the argument is itself a `mut` parameter it is already
                    // a `&mut T`; reborrow it as the closure's `&mut T` argument by
                    // passing the binding directly (`f(read u, mut ctx)` where
                    // `ctx: &mut Ctx`), exactly like a `mut`-arg to a regular
                    // function. Otherwise take `&mut` of the lowered place.
                    if let Expr::Ident(name, _) = inner
                        && self.param_effects.get(name) == Some(&DataEffect::Mut)
                    {
                        rust_value_ident(name)
                    } else {
                        format!("&mut {}", self.lower_expr(inner))
                    }
                }
                // A defaulted/`take` parameter is passed by value; a `read`
                // call-site marker without a declared effect (older value-model
                // `Fn` types) still lowers by value via `lower_owned_expr`.
                _ => self.lower_owned_expr(&arg.value),
            };
        }
        if runtime_intrinsic_wants_managed_handle_arg(callee, arg.name.as_deref())
            && let Expr::Effect { effect, value, .. } = &arg.value
            && self.expr_lowers_to_managed_handle(value)
        {
            return self.lower_managed_handle_effect_arg(*effect, value);
        }
        if self.call_arg_is_retained(callee, arg, index)
            && runtime_intrinsic_target(callee).is_none()
            && let Expr::Effect { effect, value, .. } = &arg.value
            && self.expr_lowers_to_managed_handle(value)
        {
            return self.lower_managed_handle_effect_arg(*effect, value);
        }
        // A `read`-effect argument passed to a user function's *by-value* `Copy`
        // parameter (declared with no `read`/`mut` effect, so it lowers to e.g.
        // `f64`, not `&f64`) is passed by value, not borrowed. Without this a
        // `read`-float argument was borrowed against a by-value `f64` parameter —
        // a VM↔compiler build gap. Receiver/intrinsic calls aren't `Callee::Name`,
        // so their `&T` ABI (`char_is_alpha(&char)`, …) is unaffected.
        if let Callee::Name(_) = callee
            && let Expr::Effect {
                effect: DataEffect::Read,
                value,
                ..
            } = &arg.value
            && let Some(expected) = self.expected_call_arg_type(callee, arg, index)
            && is_copy_type_ref(&expected)
            && !matches!(
                self.expected_call_arg_effect(callee, arg, index),
                Some(DataEffect::Read | DataEffect::Mut)
            )
        {
            return self.lower_expr_for_expected_type(value, &expected);
        }
        if arg.name.is_none()
            && let Some(DataEffect::Read) = self.expected_call_arg_effect(callee, arg, index)
            && let Some(expected) = self.expected_call_arg_type(callee, arg, index)
        {
            let lowered = self.lower_receiver_positional_arg(&arg.value, &expected);
            if is_copy_type_ref(&expected) {
                return lowered;
            }
            return format!("&{lowered}");
        }
        if let Some(expected) = self.expected_call_arg_type(callee, arg, index) {
            return self.lower_call_arg_for_expected_type(&arg.value, &expected);
        }
        self.lower_expr(&arg.value)
    }

    pub(super) fn lower_call_arg_for_expected_type(
        &mut self,
        value: &Expr,
        expected: &TypeRef,
    ) -> String {
        if expected.name == "Fn" {
            if let Expr::Closure { params, body, .. } = value {
                // A closure passed to a function PARAMETER typed `owned Fn` (or
                // `noescape Fn`) is consumed in-place: the parameter lowers to
                // `impl FnMut`, NOT a stored `Rc<dyn Fn>`. So this path lowers
                // the closure WITHOUT the `Rc::new` storable wrapper.
                return self.lower_closure_for_expected_fn(
                    params, body, expected, /* storable */ false,
                );
            }
            return self.lower_expr_for_expected_type(value, expected);
        }
        match value {
            Expr::Call {
                callee:
                    Callee::ReceiverCall {
                        effect: Some(DataEffect::Read) | None,
                        method,
                        ..
                    },
                ..
            } if method != "clone" && !is_copy_type_ref(expected) => {
                // `.clone()` yields an owned value (it must not be re-borrowed when passed to a
                // by-value param); every other `read` receiver-call returns a borrow-friendly result.
                format!("&{}", self.lower_expr_for_expected_type(value, expected))
            }
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                span,
                ..
            } => {
                if Self::read_effect_lowers_by_value(expected) {
                    self.lower_expr_for_expected_type(value, expected)
                } else if self.expr_lowers_to_managed_non_class_handle(value) {
                    format!(
                        "&*rsscript_runtime::unwrap_runtime({}.try_read_at({}))",
                        self.lower_expr(value),
                        lower_source_span(span)
                    )
                } else if expr_is_json_literal(value)
                    || (expected.name == "Map" && matches!(value.as_ref(), Expr::MapLiteral { .. }))
                    || (expected.name == "List"
                        && matches!(value.as_ref(), Expr::ArrayLiteral { .. }))
                {
                    format!("&{}", self.lower_expr_for_expected_type(value, expected))
                } else if let Expr::Ident(name, _) = value.as_ref()
                    && self.param_effects.get(name) == Some(&DataEffect::Read)
                {
                    // A `read`-PARAM already lowers to `&T`; passing it on as a `read`
                    // argument must NOT add another `&` (that produced `&&T` and an
                    // ill-typed call, e.g. `list_push(&mut v, &&node)`). Mirrors the
                    // `mut`-param case just below.
                    rust_value_ident(name)
                } else {
                    format!("&({})", self.lower_expr(value))
                }
            }
            Expr::Effect {
                effect: DataEffect::Mut,
                value,
                span,
                ..
            } => {
                if let Expr::Ident(name, _) = value.as_ref()
                    && self.param_effects.get(name) == Some(&DataEffect::Mut)
                {
                    rust_value_ident(name)
                } else if self.is_class_type(expected) {
                    format!("&{}", self.lower_expr_for_expected_type(value, expected))
                } else if self.expr_lowers_to_managed_non_class_handle(value) {
                    format!(
                        "&mut *rsscript_runtime::unwrap_runtime({}.try_write_at({}))",
                        self.lower_expr(value),
                        lower_source_span(span)
                    )
                } else {
                    format!(
                        "&mut {}",
                        self.lower_expr_for_expected_type(value, expected)
                    )
                }
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_expr_for_expected_type(value, expected),
            _ => self.lower_expr_for_expected_type(value, expected),
        }
    }

    pub(super) fn expected_call_arg_type(
        &self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> Option<TypeRef> {
        let key = native_boundary_callee_key(callee);
        let params = self.function_param_types.get(&key)?;
        if let Some(name) = arg.name.as_deref() {
            params
                .iter()
                .find(|(param_name, _)| param_name == name)
                .map(|(_, ty)| ty.clone())
        } else {
            params.get(index).map(|(_, ty)| ty.clone())
        }
    }

    pub(super) fn expected_call_arg_effect(
        &self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> Option<DataEffect> {
        let key = native_boundary_callee_key(callee);
        let params = self.function_param_effects.get(&key)?;
        if let Some(name) = arg.name.as_deref() {
            params
                .iter()
                .find(|(param_name, _)| param_name == name)
                .and_then(|(_, effect)| *effect)
        } else {
            params.get(index).and_then(|(_, effect)| *effect)
        }
    }

    pub(super) fn lower_capability_from_call(
        &mut self,
        protocol: &str,
        args: &[CallArg],
    ) -> String {
        let Some(value_arg) = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("value"))
            .or_else(|| args.first())
        else {
            return format!("{}::/* missing value */", capability_enum_name(protocol));
        };
        let value_type = self.infer_expr_type(&value_arg.value);
        let value_type_name = value_type
            .as_ref()
            .map(type_ref_display_name)
            .unwrap_or_else(|| "Unknown".to_string());
        let value_type_root = type_root_name(&value_type_name);
        format!(
            "{}::{}({})",
            capability_enum_name(protocol),
            rust_ident(value_type_root),
            self.lower_expr(&value_arg.value)
        )
    }

    pub(super) fn infer_call_return_type(
        &self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<TypeRef> {
        let key = native_boundary_callee_key(callee);
        let return_ty = self.function_return_types.get(&key)?.clone();
        let Some(type_params) = self.function_type_params.get(&key) else {
            return Some(return_ty);
        };
        if type_params.is_empty() {
            return Some(return_ty);
        }

        let mut substitutions = BTreeMap::new();
        self.collect_explicit_call_type_substitutions(
            callee,
            type_params,
            span,
            &mut substitutions,
        );
        self.collect_arg_type_substitutions(callee, args, &mut substitutions);

        if substitutions.is_empty() {
            Some(return_ty)
        } else {
            Some(substitute_type_ref(&return_ty, &substitutions))
        }
    }

    pub(super) fn collect_explicit_call_type_substitutions(
        &self,
        callee: &Callee,
        type_params: &[String],
        span: &Span,
        substitutions: &mut BTreeMap<String, TypeRef>,
    ) {
        let explicit = match callee {
            Callee::Name(name) | Callee::Qualified { name, .. } => type_arg_names(name),
            Callee::ReceiverCall { method, .. } => type_arg_names(method),
        };
        if let Some(explicit) = explicit {
            for (param, actual) in type_params.iter().zip(explicit) {
                substitutions
                    .entry(param.clone())
                    .or_insert_with(|| type_ref_from_display(actual, span));
            }
        }

        let Callee::Qualified { namespace, .. } = callee else {
            return;
        };
        let Some(namespace_args) = type_arg_names(namespace) else {
            return;
        };
        let Some(namespace_params) = builtin_generic_type_params(type_root_name(namespace)) else {
            return;
        };
        for (param, actual) in namespace_params.into_iter().zip(namespace_args) {
            if type_params.iter().any(|type_param| type_param == param) {
                substitutions
                    .entry(param.to_string())
                    .or_insert_with(|| type_ref_from_display(actual, span));
            }
        }
    }

    pub(super) fn collect_arg_type_substitutions(
        &self,
        callee: &Callee,
        args: &[CallArg],
        substitutions: &mut BTreeMap<String, TypeRef>,
    ) {
        let key = native_boundary_callee_key(callee);
        let Some(params) = self.function_param_types.get(&key) else {
            return;
        };
        for (index, arg) in args.iter().enumerate() {
            let Some((_, expected)) = arg
                .name
                .as_deref()
                .and_then(|name| params.iter().find(|(param_name, _)| param_name == name))
                .or_else(|| params.get(index))
            else {
                continue;
            };
            let Some(actual) = self.infer_call_arg_type(&arg.value) else {
                continue;
            };
            collect_type_ref_substitutions(expected, &actual, substitutions);
        }
    }

    pub(super) fn call_arg_is_retained(
        &self,
        callee: &Callee,
        arg: &CallArg,
        _index: usize,
    ) -> bool {
        let Some(name) = arg.name.as_deref() else {
            return false;
        };
        self.retained_params_by_callee
            .get(&native_boundary_callee_key(callee))
            .is_some_and(|retained| retained.contains(name))
    }

    pub(super) fn is_protocol_callee(&self, callee: &Callee) -> bool {
        matches!(callee, Callee::Qualified { namespace, .. } if self.protocol_names.contains(namespace))
    }

    pub(super) fn lower_return_expr(&mut self, expr: &Expr) -> String {
        let lowered = if let Some(expected) = self.current_return_type.clone() {
            self.lower_expr_for_expected_type(expr, &expected)
        } else {
            self.lower_expr(expr)
        };
        if self
            .current_return_type
            .as_ref()
            .is_some_and(is_result_type)
            && !is_result_constructor_expr(expr)
            && !self.expr_returns_result(expr)
        {
            format!("Ok({lowered})")
        } else {
            lowered
        }
    }

    pub(super) fn expr_returns_result(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Call { callee, .. } => {
                if let Callee::ReceiverCall {
                    receiver, method, ..
                } = callee
                {
                    return self
                        .infer_expr_type(receiver)
                        .map(|receiver_type| {
                            let namespace = self.receiver_call_namespace(&receiver_type, method);
                            let qualified = Callee::Qualified {
                                namespace,
                                name: method.clone(),
                            };
                            self.function_return_types
                                .get(&native_boundary_callee_key(&qualified))
                                .is_some_and(is_result_type)
                        })
                        .unwrap_or(false);
                }
                self.function_return_types
                    .get(&native_boundary_callee_key(callee))
                    .is_some_and(is_result_type)
                    || matches!(
                        callee,
                        Callee::Name(name)
                            if self
                                .value_types
                                .get(name)
                                .and_then(fn_type_return)
                                .is_some_and(is_result_type)
                    )
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
                self.expr_returns_result(value)
            }
            Expr::Match { arms, .. } => {
                arms.first().is_some_and(|arm| {
                    arm.body.statements.iter().next_back().is_some_and(
                        |statement| match statement {
                            Stmt::Return(stmt) => stmt
                                .value
                                .as_ref()
                                .is_some_and(|value| self.expr_returns_result(value)),
                            Stmt::Expr(value) => self.expr_returns_result(value),
                            _ => false,
                        },
                    )
                })
            }
            _ => false,
        }
    }

    pub(super) fn receiver_call_expected_arg_type(
        &self,
        namespace: &str,
        method: &str,
        receiver_type: Option<&TypeRef>,
        arg_name: Option<&str>,
        arg_index: usize,
    ) -> Option<TypeRef> {
        let positional_name = || {
            let qualified = Callee::Qualified {
                namespace: namespace.to_string(),
                name: method.to_string(),
            };
            let key = native_boundary_callee_key(&qualified);
            self.function_param_types
                .get(&key)?
                .get(arg_index + 1)
                .map(|(name, _)| name.as_str())
        };
        let resolved_arg_name = arg_name.or_else(positional_name);
        if type_root_name(namespace) == "List" {
            let receiver_type = receiver_type?;
            let item_type = receiver_type.args.first()?.clone();
            return match type_root_name(method) {
                "push" | "set" if resolved_arg_name == Some("value") => Some(item_type),
                "append" if resolved_arg_name == Some("values") => Some(TypeRef {
                    name: "List".to_string(),
                    args: vec![item_type],
                    malformed_arg_spans: Vec::new(),
                    is_fresh: false,
                    is_noescape: false,
                    is_owned: false,
                    fn_params: Vec::new(),
                    fn_param_effects: Vec::new(),
                    fn_return: None,
                    span: receiver_type.span.clone(),
                }),
                "join" if resolved_arg_name == Some("separator") => {
                    Some(simple_type_ref("String", &receiver_type.span))
                }
                "count_where" | "any" | "all" | "find" | "filter"
                    if resolved_arg_name == Some("predicate") =>
                {
                    Some(fn_type_ref(
                        vec![item_type],
                        Some(simple_type_ref("Bool", &receiver_type.span)),
                        &receiver_type.span,
                    ))
                }
                "map" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![item_type], None, &receiver_type.span))
                }
                "flat_map" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![item_type], None, &receiver_type.span))
                }
                "sort_with" if resolved_arg_name == Some("compare") => Some(fn_type_ref(
                    vec![item_type.clone(), item_type],
                    Some(simple_type_ref("Int", &receiver_type.span)),
                    &receiver_type.span,
                )),
                _ => None,
            };
        }
        if type_root_name(namespace) == "Result" {
            let receiver_type = receiver_type?;
            let ok_type = receiver_type.args.first()?.clone();
            let err_type = receiver_type.args.get(1)?.clone();
            return match type_root_name(method) {
                "unwrap_or" if resolved_arg_name == Some("default") => Some(ok_type),
                "map" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![ok_type], None, &receiver_type.span))
                }
                "and_then" if resolved_arg_name == Some("mapper") => Some(fn_type_ref(
                    vec![ok_type],
                    Some(TypeRef {
                        name: "Result".to_string(),
                        args: vec![simple_type_ref("Unit", &receiver_type.span), err_type],
                        malformed_arg_spans: Vec::new(),
                        is_fresh: false,
                        is_noescape: false,
                        is_owned: false,
                        fn_params: Vec::new(),
                        fn_param_effects: Vec::new(),
                        fn_return: None,
                        span: receiver_type.span.clone(),
                    }),
                    &receiver_type.span,
                )),
                _ => None,
            };
        }
        if type_root_name(namespace) == "Option" {
            let receiver_type = receiver_type?;
            let item_type = receiver_type.args.first()?.clone();
            return match type_root_name(method) {
                "unwrap_or" if resolved_arg_name == Some("default") => Some(item_type),
                "map" | "and_then" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![item_type], None, &receiver_type.span))
                }
                "filter" if resolved_arg_name == Some("predicate") => Some(fn_type_ref(
                    vec![item_type],
                    Some(simple_type_ref("Bool", &receiver_type.span)),
                    &receiver_type.span,
                )),
                _ => None,
            };
        }
        if type_root_name(namespace) == "Map" {
            let receiver_type = receiver_type?;
            let key_type = receiver_type.args.first()?.clone();
            let value_type = receiver_type.args.get(1)?.clone();
            return match type_root_name(method) {
                "contains_key" | "get" | "get_or_default" | "insert" | "insert_old" | "remove"
                    if resolved_arg_name == Some("key") =>
                {
                    Some(key_type)
                }
                "get_or_default" | "insert" | "insert_old"
                    if resolved_arg_name == Some("value") =>
                {
                    Some(value_type)
                }
                "get_or_default" if resolved_arg_name == Some("default") => Some(value_type),
                _ => None,
            };
        }
        let qualified = Callee::Qualified {
            namespace: namespace.to_string(),
            name: method.to_string(),
        };
        let key = native_boundary_callee_key(&qualified);
        if let Some(params) = self.function_param_types.get(&key) {
            if let Some(arg_name) = arg_name
                && let Some((_, ty)) = params.iter().find(|(param_name, _)| param_name == arg_name)
            {
                return Some(ty.clone());
            }
            if arg_name.is_none()
                && let Some((_, ty)) = params.get(arg_index + 1)
            {
                return Some(ty.clone());
            }
        }
        None
    }

    pub(super) fn lower_receiver_positional_arg(
        &mut self,
        value: &Expr,
        expected: &TypeRef,
    ) -> String {
        if expected.name == "String"
            && expected.args.is_empty()
            && let Expr::Ident(name, _) = value
            && self.param_effects.get(name) == Some(&DataEffect::Read)
        {
            return rust_value_ident(name);
        }
        self.lower_expr_for_expected_type(value, expected)
    }

    pub(super) fn receiver_call_namespace(&self, receiver_type: &TypeRef, method: &str) -> String {
        let receiver_type_name = type_ref_display_name(receiver_type);
        let receiver_type_root = type_root_name(&receiver_type_name).to_string();
        self.generic_protocol_bounds
            .get(&receiver_type_name)
            .cloned()
            .or_else(|| capability_protocol_name(&receiver_type_name).map(str::to_string))
            .or_else(|| self.protocol_impl_namespace(&receiver_type_root, method))
            .or_else(|| receiver_facade_namespace(&receiver_type_root, method).map(str::to_string))
            .unwrap_or(receiver_type_root)
    }

    pub(super) fn receiver_call_expected_arg_effect(
        &self,
        namespace: &str,
        method: &str,
        arg_index: usize,
    ) -> Option<DataEffect> {
        let qualified = Callee::Qualified {
            namespace: namespace.to_string(),
            name: method.to_string(),
        };
        let key = native_boundary_callee_key(&qualified);
        self.function_param_effects.get(&key)?.get(arg_index + 1)?.1
    }

    /// Builtin value types whose `.clone()` the checker resolves (via the `Clone`
    /// protocol) and which lower to a `Clone` Rust type but have no dedicated clone
    /// runtime intrinsic — so `.clone()` must lower to Rust's `.clone()` directly.
    /// Without this they fell through to a dangling `List_clone`-style call (E0425).
    /// `String`/`Json` are excluded (they have their own clone intrinsics);
    /// `Deque`/`Set`/`Map`/resources are rejected at check time, so they never reach
    /// here.
    pub(super) fn is_builtin_clone_value(name: &str) -> bool {
        matches!(name, "List" | "Bytes" | "Buffer")
    }

    pub(super) fn is_string_comparison_operand(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::String(_, _) | Expr::MultilineString(_, _))
            || self
                .infer_expr_type(expr)
                .is_some_and(|ty| ty.name == "String" && ty.args.is_empty())
    }

    pub(super) fn lower_string_comparison_operand(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::String(value, _) => format!("{:?}", decode_string_token(value)),
            Expr::MultilineString(value, _) => format!("{value:?}"),
            _ if self.is_string_comparison_operand(expr) => {
                format!("{}.as_str()", self.lower_expr(expr))
            }
            _ => self.lower_expr(expr),
        }
    }

    pub(super) fn lower_retained_expr_for_expected_type(
        &mut self,
        expr: &Expr,
        expected: &TypeRef,
    ) -> String {
        match expr {
            Expr::Ident(name, _) if !is_copy_type_ref(expected) => {
                format!("{}.clone()", rust_value_ident(name))
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_owned_expr_for_expected_type(value, expected),
            _ => self.lower_owned_expr_for_expected_type(expr, expected),
        }
    }

    pub(super) fn lower_owned_expr_for_expected_type(
        &mut self,
        expr: &Expr,
        expected: &TypeRef,
    ) -> String {
        match expr {
            Expr::Ident(name, _)
                if !is_copy_type_ref(expected)
                    && matches!(
                        self.param_effects.get(name),
                        Some(DataEffect::Read | DataEffect::Mut)
                    ) =>
            {
                format!("{}.clone()", rust_value_ident(name))
            }
            Expr::Effect {
                effect: DataEffect::Read | DataEffect::Mut,
                value,
                ..
            } => {
                let lowered = self.lower_expr_for_expected_type(value, expected);
                if is_copy_type_ref(expected) {
                    lowered
                } else {
                    format!("{lowered}.clone()")
                }
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_expr_for_expected_type(value, expected),
            _ => self.lower_expr_for_expected_type(expr, expected),
        }
    }

    pub(super) fn constructor_field_arg_name(
        &self,
        type_name: &str,
        arg: &CallArg,
    ) -> Option<String> {
        if let Some(name) = arg.name.as_deref() {
            return Some(name.to_string());
        }
        let Expr::Ident(name, _) = &arg.value else {
            return None;
        };
        self.field_type(type_name, name).map(|_| name.clone())
    }

    pub(super) fn field_type(&self, type_name: &str, field_name: &str) -> Option<TypeRef> {
        self.program.items.iter().find_map(|item| match item {
            Item::Type(ty) if ty.name == type_name => ty
                .fields
                .iter()
                .find(|field| field.name == field_name)
                .map(|field| field.ty.clone()),
            _ => None,
        })
    }

    pub(super) fn is_resource_type(&self, ty: &TypeRef) -> bool {
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Resource))
    }
}

/// Whether any arm matches with a list slice pattern, so the scrutinee must be
/// lowered as a `[T]` slice rather than a `Vec<T>`.
pub(super) fn arms_have_list_pattern(arms: &[crate::syntax::ast::MatchArm]) -> bool {
    arms.iter()
        .any(|arm| matches!(arm.pattern, MatchPattern::List { .. }))
}

pub(super) fn match_pattern_span(pattern: &MatchPattern) -> Span {
    match pattern {
        MatchPattern::Variant { span, .. }
        | MatchPattern::Struct { span, .. }
        | MatchPattern::Literal { span, .. }
        | MatchPattern::List { span, .. }
        | MatchPattern::Binding { span, .. }
        | MatchPattern::Wildcard(span) => span.clone(),
    }
}

pub(super) fn capability_enum_name(protocol: &str) -> String {
    format!("Capability{}", rust_ident(protocol))
}

pub(super) fn capability_impl_forward_arg(param: &Param) -> String {
    if param.name == "self" {
        "inner".to_string()
    } else {
        rust_value_ident(&param.name)
    }
}

pub(super) fn capability_from_protocol(callee: &Callee) -> Option<&str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    if type_root_name(namespace) != "Capability" || type_root_name(name) != "from" {
        return None;
    }
    type_arg_names(namespace).and_then(|args| args.first().copied())
}

pub(super) fn capability_protocol_name(type_name: &str) -> Option<&str> {
    if type_root_name(type_name) != "Capability" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().copied())
}

pub(super) fn type_ref_display_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }
    format!(
        "{}<{}>",
        ty.name,
        ty.args
            .iter()
            .map(type_ref_display_name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn closure_binding(name: &str, is_mut: bool) -> String {
    let name = rust_ident(name);
    if is_mut { format!("mut {name}") } else { name }
}

pub(super) fn awaited_binding_type(value_type: Option<TypeRef>, is_try: bool) -> Option<TypeRef> {
    let value_type = value_type?;
    if is_try {
        result_ok_type_ref(&value_type)
    } else {
        Some(value_type)
    }
}

pub(super) fn stmt_contains_await(statement: &Stmt) -> bool {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().is_some_and(expr_contains_await),
        Stmt::Return(stmt) => stmt.value.as_ref().is_some_and(expr_contains_await),
        Stmt::With(stmt) => expr_contains_await(&stmt.resource) || block_contains_await(&stmt.body),
        Stmt::If(stmt) => {
            expr_contains_await(&stmt.condition)
                || block_contains_await(&stmt.then_body)
                || stmt.else_body.as_ref().is_some_and(block_contains_await)
        }
        Stmt::Loop(stmt) => {
            stmt.condition.as_ref().is_some_and(expr_contains_await)
                || block_contains_await(&stmt.body)
        }
        Stmt::For(stmt) => {
            stmt.is_async || expr_contains_await(&stmt.iterable) || block_contains_await(&stmt.body)
        }
        Stmt::TaskGroup(_) | Stmt::Select(_) => true,
        Stmt::Match(stmt) => {
            expr_contains_await(&stmt.value)
                || stmt.arms.iter().any(|arm| block_contains_await(&arm.body))
        }
        Stmt::LetElse(stmt) => {
            expr_contains_await(&stmt.value) || block_contains_await(&stmt.else_body)
        }
        Stmt::Assign(stmt) => expr_contains_await(&stmt.target) || expr_contains_await(&stmt.value),
        Stmt::Expr(expr) => expr_contains_await(expr),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => false,
    }
}

pub(super) fn block_contains_await(block: &Block) -> bool {
    block.statements.iter().any(stmt_contains_await)
}

pub(super) fn stmt_contains_try(statement: &Stmt) -> bool {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().is_some_and(expr_contains_try),
        Stmt::Return(stmt) => stmt.value.as_ref().is_some_and(expr_contains_try),
        Stmt::With(stmt) => expr_contains_try(&stmt.resource) || block_contains_try(&stmt.body),
        Stmt::If(stmt) => {
            expr_contains_try(&stmt.condition)
                || block_contains_try(&stmt.then_body)
                || stmt.else_body.as_ref().is_some_and(block_contains_try)
        }
        Stmt::Loop(stmt) => {
            stmt.condition.as_ref().is_some_and(expr_contains_try) || block_contains_try(&stmt.body)
        }
        Stmt::For(stmt) => expr_contains_try(&stmt.iterable) || block_contains_try(&stmt.body),
        Stmt::Match(stmt) => {
            expr_contains_try(&stmt.value)
                || stmt.arms.iter().any(|arm| block_contains_try(&arm.body))
        }
        Stmt::LetElse(stmt) => {
            expr_contains_try(&stmt.value) || block_contains_try(&stmt.else_body)
        }
        Stmt::Assign(stmt) => expr_contains_try(&stmt.target) || expr_contains_try(&stmt.value),
        Stmt::Expr(expr) => expr_contains_try(expr),
        Stmt::TaskGroup(_)
        | Stmt::Select(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => false,
    }
}

pub(super) fn block_contains_try(block: &Block) -> bool {
    block.statements.iter().any(stmt_contains_try)
}

/// Substitute concrete `args` for the generic type parameters `params` in `ty`.
/// A declared field type that names a parameter (e.g. `A` in `__Tuple2<A, B>`)
/// resolves to the matching argument from the scrutinee's `value_type`; nested
/// type arguments are substituted recursively. Unresolved names are left as-is
/// (treated as non-`Copy`, cloneable values).
pub(super) fn substitute_generic_type(
    ty: &TypeRef,
    params: &[String],
    args: &[TypeRef],
) -> TypeRef {
    if ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && let Some(index) = params.iter().position(|param| param == &ty.name)
        && let Some(arg) = args.get(index)
    {
        return arg.clone();
    }
    let mut resolved = ty.clone();
    resolved.args = ty
        .args
        .iter()
        .map(|arg| substitute_generic_type(arg, params, args))
        .collect();
    resolved.fn_params = ty
        .fn_params
        .iter()
        .map(|param| substitute_generic_type(param, params, args))
        .collect();
    resolved.fn_return = ty
        .fn_return
        .as_ref()
        .map(|ret| Box::new(substitute_generic_type(ret, params, args)));
    resolved
}

pub(super) fn match_binding_type_ref(
    pattern: &MatchPattern,
    value_type: Option<&TypeRef>,
) -> Option<(String, TypeRef)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.cloned().map(|ty| (name.clone(), ty));
    }
    let MatchPattern::Variant {
        name,
        binding: Some(binding),
        ..
    } = pattern
    else {
        return None;
    };
    let value_type = value_type?;
    match name.as_str() {
        "Some" if value_type.name == "Option" => value_type
            .args
            .first()
            .cloned()
            .and_then(|ty| match_binding_type_ref(binding, Some(&ty))),
        "Ok" if value_type.name == "Result" => value_type
            .args
            .first()
            .cloned()
            .and_then(|ty| match_binding_type_ref(binding, Some(&ty))),
        "Err" if value_type.name == "Result" => value_type
            .args
            .get(1)
            .cloned()
            .and_then(|ty| match_binding_type_ref(binding, Some(&ty))),
        _ => None,
    }
}

pub(super) fn split_loop_body_at_first_await(
    statements: &[Stmt],
) -> Option<(&[Stmt], &Stmt, &[Stmt])> {
    for (index, statement) in statements.iter().enumerate() {
        match statement {
            Stmt::Let(stmt) if stmt.value.as_ref().and_then(async_await_inner).is_some() => {
                return Some((&statements[..index], statement, &statements[index + 1..]));
            }
            Stmt::Expr(expr) if async_await_inner(expr).is_some() => {
                return Some((&statements[..index], statement, &statements[index + 1..]));
            }
            other if stmt_contains_await(other) => return None,
            _ => {}
        }
    }
    None
}

pub(super) fn expr_contains_await(expr: &Expr) -> bool {
    match expr {
        Expr::Await { .. } => true,
        Expr::Try { value, .. }
        | Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. } => expr_contains_await(value),
        Expr::Binary { left, right, .. } => expr_contains_await(left) || expr_contains_await(right),
        Expr::Call { args, .. } => args.iter().any(|arg| expr_contains_await(&arg.value)),
        Expr::Field { base, .. } => expr_contains_await(base),
        Expr::Index { base, index, .. } => expr_contains_await(base) || expr_contains_await(index),
        Expr::Closure { body, .. } => block_contains_await(body),
        Expr::Match { value, arms, .. } => {
            expr_contains_await(value) || arms.iter().any(|arm| block_contains_await(&arm.body))
        }
        Expr::ObjectLiteral { fields, .. } => {
            fields.iter().any(|field| expr_contains_await(&field.value))
        }
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .any(|entry| expr_contains_await(&entry.key) || expr_contains_await(&entry.value)),
        Expr::ArrayLiteral { items, .. } => items.iter().any(expr_contains_await),
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => false,
    }
}

pub(super) fn expr_contains_try(expr: &Expr) -> bool {
    match expr {
        Expr::Try { .. } => true,
        Expr::Await { value, .. }
        | Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. } => expr_contains_try(value),
        Expr::Binary { left, right, .. } => expr_contains_try(left) || expr_contains_try(right),
        Expr::Call { args, .. } => args.iter().any(|arg| expr_contains_try(&arg.value)),
        Expr::Field { base, .. } => expr_contains_try(base),
        Expr::Index { base, index, .. } => expr_contains_try(base) || expr_contains_try(index),
        Expr::Closure { body, .. } => block_contains_try(body),
        Expr::Match { value, arms, .. } => {
            expr_contains_try(value) || arms.iter().any(|arm| block_contains_try(&arm.body))
        }
        Expr::ObjectLiteral { fields, .. } => {
            fields.iter().any(|field| expr_contains_try(&field.value))
        }
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .any(|entry| expr_contains_try(&entry.key) || expr_contains_try(&entry.value)),
        Expr::ArrayLiteral { items, .. } => items.iter().any(expr_contains_try),
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => false,
    }
}

/// Join already-lowered operands with a short-circuit operator (`&&`/`||`) as a
/// *balanced* tree, so the resulting Rust expression nests O(log n) deep rather
/// than O(n). `&&`/`||` are associative for both value and evaluation order, so
/// any grouping evaluates the operands left-to-right with identical short-circuit
/// behavior. Recursion depth here is O(log n), so this helper is itself safe.
pub(super) fn balanced_logical_join(leaves: &[String], op: &str) -> String {
    match leaves.len() {
        0 => String::new(),
        1 => leaves[0].clone(),
        n => {
            let mid = n / 2;
            format!(
                "({} {op} {})",
                balanced_logical_join(&leaves[..mid], op),
                balanced_logical_join(&leaves[mid..], op)
            )
        }
    }
}

pub(super) fn rust_binary_precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::LogicalOr => 1,
        BinaryOp::LogicalAnd => 2,
        BinaryOp::BitOr => 3,
        BinaryOp::BitXor => 4,
        BinaryOp::BitAnd => 5,
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual => 6,
        BinaryOp::ShiftLeft | BinaryOp::ShiftRight => 7,
        BinaryOp::Add | BinaryOp::Subtract => 8,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 9,
    }
}

pub(super) fn rust_binary_is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
    )
}

pub(super) fn type_ref_from_display(name: &str, span: &Span) -> TypeRef {
    let (name, is_fresh) = name
        .trim()
        .strip_prefix("fresh ")
        .map_or((name.trim(), false), |name| (name.trim(), true));
    TypeRef {
        name: type_root_name(name).to_string(),
        args: type_arg_names(name)
            .unwrap_or_default()
            .into_iter()
            .map(|arg| type_ref_from_display(arg, span))
            .collect(),
        malformed_arg_spans: Vec::new(),
        is_fresh,
        is_noescape: false,
        is_owned: false,
        fn_params: Vec::new(),
        fn_param_effects: Vec::new(),
        fn_return: None,
        span: span.clone(),
    }
}

pub(super) fn simple_type_ref(name: &str, span: &Span) -> TypeRef {
    TypeRef {
        name: name.to_string(),
        args: Vec::new(),
        malformed_arg_spans: Vec::new(),
        is_fresh: false,
        is_noescape: false,
        is_owned: false,
        fn_params: Vec::new(),
        fn_param_effects: Vec::new(),
        fn_return: None,
        span: span.clone(),
    }
}

pub(super) fn substitute_type_ref(
    ty: &TypeRef,
    substitutions: &BTreeMap<String, TypeRef>,
) -> TypeRef {
    if ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && let Some(replacement) = substitutions.get(&ty.name)
    {
        let mut replacement = replacement.clone();
        replacement.is_fresh |= ty.is_fresh;
        replacement.is_noescape |= ty.is_noescape;
        replacement.is_owned |= ty.is_owned;
        return replacement;
    }

    let mut replaced = ty.clone();
    replaced.args = ty
        .args
        .iter()
        .map(|arg| substitute_type_ref(arg, substitutions))
        .collect();
    replaced.fn_params = ty
        .fn_params
        .iter()
        .map(|param| substitute_type_ref(param, substitutions))
        .collect();
    replaced.fn_return = ty
        .fn_return
        .as_ref()
        .map(|return_ty| Box::new(substitute_type_ref(return_ty, substitutions)));
    replaced
}

pub(super) fn collect_type_ref_substitutions(
    pattern: &TypeRef,
    actual: &TypeRef,
    substitutions: &mut BTreeMap<String, TypeRef>,
) {
    if pattern.args.is_empty() && pattern.fn_params.is_empty() && pattern.fn_return.is_none() {
        match pattern.name.as_str() {
            "T" | "K" | "V" | "E" | "P" => {
                substitutions
                    .entry(pattern.name.clone())
                    .or_insert_with(|| actual.clone());
                return;
            }
            _ => {}
        }
    }

    if pattern.name != actual.name || pattern.args.len() != actual.args.len() {
        return;
    }
    for (pattern_arg, actual_arg) in pattern.args.iter().zip(actual.args.iter()) {
        collect_type_ref_substitutions(pattern_arg, actual_arg, substitutions);
    }
    for (pattern_param, actual_param) in pattern.fn_params.iter().zip(actual.fn_params.iter()) {
        collect_type_ref_substitutions(pattern_param, actual_param, substitutions);
    }
    if let (Some(pattern_return), Some(actual_return)) =
        (pattern.fn_return.as_ref(), actual.fn_return.as_ref())
    {
        collect_type_ref_substitutions(pattern_return, actual_return, substitutions);
    }
}

use crate::text_util::builtin_generic_type_params;

/// Type names declared by the bundled stdlib (`builtin`) interfaces. These are
/// runtime-backed (lowered as `rsscript_runtime::X`), so they must be kept out of
/// the per-package `type_kinds` map even though they appear among the interface
/// programs — otherwise they would be reclassified as local user types. Parsed
/// once and cached (the builtin interface set is fixed at compile time).
pub(super) fn builtin_interface_type_names() -> &'static std::collections::HashSet<String> {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        crate::interfaces::builtin_interfaces()
            .flat_map(|(file, source)| crate::syntax::parse_source(file, source).items.into_iter())
            .filter_map(|item| match item {
                Item::Type(ty) => Some(ty.name),
                _ => None,
            })
            .collect()
    })
}

pub(super) fn fn_type_ref(
    params: Vec<TypeRef>,
    return_ty: Option<TypeRef>,
    span: &Span,
) -> TypeRef {
    TypeRef {
        name: "Fn".to_string(),
        args: Vec::new(),
        malformed_arg_spans: Vec::new(),
        is_fresh: false,
        is_noescape: true,
        is_owned: false,
        fn_param_effects: vec![None; params.len()],
        fn_params: params,
        fn_return: return_ty.map(Box::new),
        span: span.clone(),
    }
}

pub(super) fn is_json_encode_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Json" && type_root_name(name) == "encode"
    )
}

pub(super) fn is_json_decode_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Json"
                && matches!(type_root_name(name), "decode" | "decode_text")
    )
}

pub(super) fn is_json_decode_text_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Json" && type_root_name(name) == "decode_text"
    )
}

pub(super) fn json_decode_type_arg(callee: &Callee) -> Option<&str> {
    match callee {
        Callee::Qualified { name, .. } => {
            type_arg_names(name).and_then(|args| args.first().copied())
        }
        Callee::Name(name) => type_arg_names(name).and_then(|args| args.first().copied()),
        Callee::ReceiverCall { method, .. } => {
            type_arg_names(method).and_then(|args| args.first().copied())
        }
    }
}

pub(super) fn expr_is_json_literal(expr: &Expr) -> bool {
    match expr {
        Expr::ObjectLiteral { .. } | Expr::ArrayLiteral { .. } => true,
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => expr_is_json_literal(value),
        _ => false,
    }
}

pub(super) fn receiver_facade_namespace(receiver_root: &str, method: &str) -> Option<&'static str> {
    match receiver_root {
        "JsonValue" | "JsonLiteral" => Some("Json"),
        "String" if method.starts_with("json_") => Some("Json"),
        _ => None,
    }
}

pub(super) fn generated_line_count(generated: &str) -> usize {
    generated
        .chars()
        .filter(|character| *character == '\n')
        .count()
        + 1
}

pub(super) fn push_unique_derive(derives: &mut Vec<String>, derive: &str) {
    if !derives.iter().any(|existing| existing == derive) {
        derives.push(derive.to_string());
    }
}

/// Whether a type reference is (or directly is) a first-class closure value —
/// an `owned Fn(...)` — which lowers to `Rc<dyn Fn>`. Such a field makes the
/// containing type underivable for `Debug`/`Eq`/`Hash`/`Ord`.
pub(super) fn type_ref_holds_closure(ty: &TypeRef) -> bool {
    ty.is_owned && ty.name == "Fn"
}

/// A manual `Debug` impl for a struct that holds a non-`Debug` `owned Fn`
/// (`Rc<dyn Fn>`) field. Function-value fields print as `<fn>`; every other
/// field defers to its own `Debug`. Keeps the auto-`Debug` contract for
/// closure-holding structs (which cannot `#[derive(Debug)]`).
pub(super) fn manual_struct_debug_impl(
    name: &str,
    type_params: &[GenericParam],
    fields: &[FieldDecl],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "impl{} std::fmt::Debug for {}{} {{\n",
        lower_impl_generics(type_params),
        name,
        lower_generic_args(type_params)
    ));
    out.push_str("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n");
    out.push_str(&format!("        f.debug_struct({name:?})\n"));
    for field in fields {
        let field_ident = rust_ident(&field.name);
        if type_ref_holds_closure(&field.ty) {
            out.push_str(&format!(
                "            .field({:?}, &\"<fn>\")\n",
                field.name
            ));
        } else {
            out.push_str(&format!(
                "            .field({:?}, &self.{field_ident})\n",
                field.name
            ));
        }
    }
    out.push_str("            .finish()\n");
    out.push_str("    }\n}\n");
    out
}

/// The Rust integer-literal suffix for an RSScript integer type name, e.g.
/// `Int -> "i64"`, `Int32 -> "i32"`. `None` for non-integer types. Used so an
/// integer literal in a typed context (sized ints, or the default `Int`) lowers
/// with the matching suffix instead of Rust's untyped-`i32` default.
pub(super) fn rust_int_literal_suffix(type_name: &str) -> Option<&'static str> {
    Some(match type_name {
        "Int" | "Int64" | "Fd" => "i64",
        "Int8" => "i8",
        "Int16" => "i16",
        "Int32" => "i32",
        "UInt" | "UInt64" => "u64",
        "UInt8" | "Byte" => "u8",
        "UInt16" => "u16",
        "UInt32" => "u32",
        _ => return None,
    })
}

pub(super) fn infer_const_type(expr: &Expr) -> String {
    match expr {
        Expr::Number(s, _) => {
            if s.contains('.') {
                "f64".to_string()
            } else {
                "i64".to_string()
            }
        }
        Expr::String(_, _) | Expr::MultilineString(_, _) => "&'static str".to_string(),
        Expr::Ident(name, _) if name == "true" || name == "false" => "bool".to_string(),
        // Non-literal const initializers are rejected by the frontend (RS0015); a
        // value reaching here means that check regressed — fail loudly.
        other => unreachable_lowering("const type", other.span()),
    }
}

pub(super) fn lower_const_value(expr: &Expr) -> String {
    match expr {
        Expr::Number(value, _) => value.clone(),
        Expr::String(value, _) => {
            format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
        }
        Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Ident(name, _) if name == "true" || name == "false" => name.clone(),
        // The frontend rejects non-literal const initializers (RS0015), so anything
        // else here is a missing front-end check — fail loudly rather than emit a
        // `()` placeholder that leaks an unmappable backend type error.
        other => unreachable_lowering("const initializer", other.span()),
    }
}

pub(super) fn is_try_wrapped(expr: &Expr) -> bool {
    matches!(expr, Expr::Try { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_generic_type_params_use_each_type_s_own_param_names() {
        // Regression: `Result` was mapped to `["K", "V"]` (Map's params), so the
        // namespace/type-argument substitution path never bound `Result`'s real
        // `T`/`E` params — weakening generic substitution for `Result.map` /
        // `map_error` / `and_then`. Each type must use its own declared param names.
        assert_eq!(builtin_generic_type_params("Map"), Some(vec!["K", "V"]));
        assert_eq!(builtin_generic_type_params("Result"), Some(vec!["T", "E"]));
        assert_eq!(builtin_generic_type_params("List"), Some(vec!["T"]));
        assert_eq!(builtin_generic_type_params("Option"), Some(vec!["T"]));
        assert_eq!(builtin_generic_type_params("NotAGeneric"), None);
    }
}
