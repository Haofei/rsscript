//! Source-preserving AST desugarings applied by [`super::parse_source`] (but not
//! [`super::parse_source_raw`], so the formatter and symbol index still see the
//! written surface).
//!
//! ## Associated constants
//!
//! `const Device.DEFAULT: String = "cpu"` declares a *type-associated* constant,
//! referenced as `Device.DEFAULT`. The reference parses as a field access
//! (`Field { base: Ident("Device"), name: "DEFAULT" }`), which no value defines.
//! Rather than teach the checker and both backends to resolve such field
//! accesses, this pass rewrites associated constants into ordinary ones:
//!
//!   * the declaration `const Device.DEFAULT` becomes `const Device_DEFAULT`, and
//!   * every reference `Device.DEFAULT` becomes the ident `Device_DEFAULT`.
//!
//! After this, all existing const machinery (resolution, type inference, VM, Rust
//! lowering) handles them with no further changes. The pass is a no-op for
//! programs without dotted const names.

use std::collections::{BTreeSet, HashMap};

use super::ast::{
    Block, Callee, Expr, FieldDecl, GenericParam, Item, Program, Stmt, TypeDecl, TypeKind, TypeRef,
};
use crate::diagnostic::Span;

/// Flatten the associated name `Device.DEFAULT` to an ordinary constant
/// identifier. Constants follow Rust's `SCREAMING_SNAKE_CASE` (the Rust backend
/// upper-cases const declaration names), so the flattened name is upper-cased too
/// — keeping the declaration, every reference, and the VM all in agreement.
fn flatten(name: &str) -> String {
    name.replace('.', "_").to_uppercase()
}

pub(crate) fn desugar_associated_consts(program: &mut Program) {
    // Map each associated (dotted) const name to its flattened form.
    let assoc: HashMap<String, String> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Const(decl) if decl.name.contains('.') => {
                Some((decl.name.clone(), flatten(&decl.name)))
            }
            _ => None,
        })
        .collect();
    if assoc.is_empty() {
        return;
    }

    for item in &mut program.items {
        match item {
            Item::Const(decl) => {
                if let Some(flat) = assoc.get(&decl.name) {
                    decl.name = flat.clone();
                }
                rewrite_expr(&mut decl.value, &assoc);
            }
            Item::Function(function) => rewrite_block(&mut function.body, &assoc),
            _ => {}
        }
    }
}

fn rewrite_block(block: &mut Block, assoc: &HashMap<String, String>) {
    for statement in &mut block.statements {
        rewrite_stmt(statement, assoc);
    }
}

fn rewrite_stmt(statement: &mut Stmt, assoc: &HashMap<String, String>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &mut stmt.value {
                rewrite_expr(value, assoc);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &mut stmt.value {
                rewrite_expr(value, assoc);
            }
        }
        Stmt::Expr(expr) => rewrite_expr(expr, assoc),
        Stmt::Assign(stmt) => {
            rewrite_expr(&mut stmt.target, assoc);
            rewrite_expr(&mut stmt.value, assoc);
        }
        Stmt::With(stmt) => {
            rewrite_expr(&mut stmt.resource, assoc);
            rewrite_block(&mut stmt.body, assoc);
        }
        Stmt::If(stmt) => {
            rewrite_expr(&mut stmt.condition, assoc);
            rewrite_block(&mut stmt.then_body, assoc);
            if let Some(else_body) = &mut stmt.else_body {
                rewrite_block(else_body, assoc);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &mut stmt.condition {
                rewrite_expr(condition, assoc);
            }
            rewrite_block(&mut stmt.body, assoc);
        }
        Stmt::For(stmt) => {
            rewrite_expr(&mut stmt.iterable, assoc);
            rewrite_block(&mut stmt.body, assoc);
        }
        Stmt::LetElse(stmt) => {
            rewrite_expr(&mut stmt.value, assoc);
            rewrite_block(&mut stmt.else_body, assoc);
        }
        Stmt::TaskGroup(stmt) => rewrite_block(&mut stmt.body, assoc),
        Stmt::Match(stmt) => {
            rewrite_expr(&mut stmt.value, assoc);
            for arm in &mut stmt.arms {
                if let Some(guard) = &mut arm.guard {
                    rewrite_expr(guard, assoc);
                }
                rewrite_block(&mut arm.body, assoc);
            }
        }
        Stmt::Select(stmt) => {
            for arm in &mut stmt.arms {
                rewrite_expr(&mut arm.operation, assoc);
                rewrite_block(&mut arm.body, assoc);
            }
        }
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

fn rewrite_expr(expr: &mut Expr, assoc: &HashMap<String, String>) {
    // An associated-const reference is a field access on a type name: rewrite the
    // whole expression to the flattened ident and stop (the "base" is the type,
    // not a value to recurse into).
    if let Expr::Field { base, name, span } = expr
        && let Expr::Ident(type_name, _) = base.as_ref()
        && let Some(flat) = assoc.get(&format!("{type_name}.{name}"))
    {
        *expr = Expr::Ident(flat.clone(), span.clone());
        return;
    }

    match expr {
        Expr::Field { base, .. } => rewrite_expr(base, assoc),
        Expr::Index { base, index, .. } => {
            rewrite_expr(base, assoc);
            rewrite_expr(index, assoc);
        }
        Expr::Binary { left, right, .. } => {
            rewrite_expr(left, assoc);
            rewrite_expr(right, assoc);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => rewrite_expr(value, assoc),
        Expr::Call { callee, args, .. } => {
            if let Callee::ReceiverCall { receiver, .. } = callee {
                rewrite_expr(receiver, assoc);
            }
            for arg in args {
                rewrite_expr(&mut arg.value, assoc);
            }
        }
        Expr::Closure { body, .. } => rewrite_block(body, assoc),
        Expr::Match { value, arms, .. } => {
            rewrite_expr(value, assoc);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    rewrite_expr(guard, assoc);
                }
                rewrite_block(&mut arm.body, assoc);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                rewrite_expr(&mut field.value, assoc);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                rewrite_expr(&mut entry.key, assoc);
                rewrite_expr(&mut entry.value, assoc);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                rewrite_expr(item, assoc);
            }
        }
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => {}
    }
}

use super::ast::{LetKind, LetStmt};

/// Expand `let (a, b) = expr` into a temporary binding plus per-element field
/// reads (`let __t = expr; let a = __t.item0; let b = __t.item1`). Runs after
/// tuple literals/types are desugared, so the temporary's value is an ordinary
/// `__TupleN` whose `.itemN` projections type-check on every backend.
pub(crate) fn expand_tuple_destructuring(program: &mut Program) {
    let mut counter = 0usize;
    for item in &mut program.items {
        match item {
            Item::Function(function) => expand_block(&mut function.body, &mut counter),
            Item::Type(decl) => {
                if let Some(body) = &mut decl.drop_body {
                    expand_block(body, &mut counter);
                }
            }
            _ => {}
        }
    }
}

fn expand_block(block: &mut Block, counter: &mut usize) {
    let mut expanded = Vec::with_capacity(block.statements.len());
    for mut statement in std::mem::take(&mut block.statements) {
        expand_stmt_nested(&mut statement, counter);
        match statement {
            Stmt::Let(let_stmt)
                if let Some(names) = let_stmt.destructure.clone()
                    && let Some(value) = let_stmt.value.clone()
                    && !let_stmt.malformed =>
            {
                let span = let_stmt.span.clone();
                let temp = format!("__rss_destructure_{}", *counter);
                *counter += 1;
                expanded.push(Stmt::Let(LetStmt {
                    kind: let_stmt.kind,
                    name: temp.clone(),
                    type_annotation: None,
                    value: Some(value),
                    is_async: false,
                    is_mut: false,
                    destructure: None,
                    malformed: false,
                    span: span.clone(),
                }));
                for (index, name) in names.iter().enumerate() {
                    if name == "_" {
                        continue;
                    }
                    let element = Expr::Field {
                        base: Box::new(Expr::Ident(temp.clone(), span.clone())),
                        name: format!("item{index}"),
                        span: span.clone(),
                    };
                    expanded.push(Stmt::Let(LetStmt {
                        kind: LetKind::Managed,
                        name: name.clone(),
                        type_annotation: None,
                        value: Some(element),
                        is_async: false,
                        is_mut: false,
                        destructure: None,
                        malformed: false,
                        span: span.clone(),
                    }));
                }
            }
            other => expanded.push(other),
        }
    }
    block.statements = expanded;
}

fn expand_stmt_nested(statement: &mut Stmt, counter: &mut usize) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &mut stmt.value {
                expand_expr_nested(value, counter);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &mut stmt.value {
                expand_expr_nested(value, counter);
            }
        }
        Stmt::Expr(expr) => expand_expr_nested(expr, counter),
        Stmt::Assign(stmt) => {
            expand_expr_nested(&mut stmt.target, counter);
            expand_expr_nested(&mut stmt.value, counter);
        }
        Stmt::With(stmt) => {
            expand_expr_nested(&mut stmt.resource, counter);
            expand_block(&mut stmt.body, counter);
        }
        Stmt::If(stmt) => {
            expand_expr_nested(&mut stmt.condition, counter);
            expand_block(&mut stmt.then_body, counter);
            if let Some(else_body) = &mut stmt.else_body {
                expand_block(else_body, counter);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &mut stmt.condition {
                expand_expr_nested(condition, counter);
            }
            expand_block(&mut stmt.body, counter);
        }
        Stmt::For(stmt) => {
            expand_expr_nested(&mut stmt.iterable, counter);
            expand_block(&mut stmt.body, counter);
        }
        Stmt::LetElse(stmt) => {
            expand_expr_nested(&mut stmt.value, counter);
            expand_block(&mut stmt.else_body, counter);
        }
        Stmt::TaskGroup(stmt) => expand_block(&mut stmt.body, counter),
        Stmt::Match(stmt) => {
            expand_expr_nested(&mut stmt.value, counter);
            for arm in &mut stmt.arms {
                if let Some(guard) = &mut arm.guard {
                    expand_expr_nested(guard, counter);
                }
                expand_block(&mut arm.body, counter);
            }
        }
        Stmt::Select(stmt) => {
            for arm in &mut stmt.arms {
                expand_expr_nested(&mut arm.operation, counter);
                expand_block(&mut arm.body, counter);
            }
        }
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

fn expand_expr_nested(expr: &mut Expr, counter: &mut usize) {
    match expr {
        Expr::Closure { body, .. } => expand_block(body, counter),
        Expr::Field { base, .. } => expand_expr_nested(base, counter),
        Expr::Index { base, index, .. } => {
            expand_expr_nested(base, counter);
            expand_expr_nested(index, counter);
        }
        Expr::Binary { left, right, .. } => {
            expand_expr_nested(left, counter);
            expand_expr_nested(right, counter);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => expand_expr_nested(value, counter),
        Expr::Call { callee, args, .. } => {
            if let Callee::ReceiverCall { receiver, .. } = callee {
                expand_expr_nested(receiver, counter);
            }
            for arg in args {
                expand_expr_nested(&mut arg.value, counter);
            }
        }
        Expr::Match { value, arms, .. } => {
            expand_expr_nested(value, counter);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    expand_expr_nested(guard, counter);
                }
                expand_block(&mut arm.body, counter);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                expand_expr_nested(&mut field.value, counter);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                expand_expr_nested(&mut entry.key, counter);
                expand_expr_nested(&mut entry.value, counter);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                expand_expr_nested(item, counter);
            }
        }
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => {}
    }
}

/// A synthetic span for desugar-injected declarations. They have no written
/// source location; diagnostics never point here because these decls are valid
/// by construction.
fn synthetic_span() -> Span {
    Span {
        file: "<tuple>".to_string(),
        line: 0,
        column: 0,
        length: 0,
    }
}

/// Tuples desugar to synthetic generic structs `__TupleN<T0, .., Tn-1>` with
/// fields `item0 .. itemN-1`. The parser already rewrites tuple literals
/// (`(a, b)` → `__Tuple2(item0: a, item1: b)`) and tuple types (`(T, U)` →
/// `__Tuple2<T, U>`); this pass scans the program for the arities actually used
/// and injects the matching struct declarations so the checker, HIR, VM, and
/// Rust backend resolve them like any other generic struct.
pub(crate) fn inject_tuple_structs(program: &mut Program) {
    let mut arities = BTreeSet::new();
    for item in &program.items {
        collect_item_arities(item, &mut arities);
    }
    // Prepend so the declarations precede every use (order is irrelevant to the
    // checker, but keeps generated output readable).
    for arity in arities.into_iter().rev() {
        program.items.insert(0, make_tuple_struct(arity));
    }
}

/// Parse `__TupleN` to its arity `N`.
fn tuple_arity(name: &str) -> Option<usize> {
    name.strip_prefix("__Tuple").and_then(|n| n.parse().ok())
}

/// The type-parameter name for tuple element `i`. The checker recognises a
/// generic type variable only as a single uppercase letter (the v0.7
/// convention), so tuple parameters are `A`, `B`, `C`, … — capping practical
/// tuple arity at 26, far above any reasonable use.
fn tuple_type_param(index: usize) -> String {
    ((b'A' + index as u8) as char).to_string()
}

fn make_tuple_struct(arity: usize) -> Item {
    let span = synthetic_span();
    let type_params: Vec<GenericParam> = (0..arity)
        .map(|i| GenericParam {
            name: tuple_type_param(i),
            bound: None,
            span: span.clone(),
        })
        .collect();
    let fields: Vec<FieldDecl> = (0..arity)
        .map(|i| FieldDecl {
            name: format!("item{i}"),
            ty: TypeRef {
                name: tuple_type_param(i),
                args: Vec::new(),
                malformed_arg_spans: Vec::new(),
                is_fresh: false,
                is_noescape: false,
                is_owned: false,
                fn_params: Vec::new(),
                fn_param_effects: Vec::new(),
                fn_return: None,
                span: span.clone(),
            },
            is_handle: false,
            is_weak: false,
            default: None,
            span: span.clone(),
        })
        .collect();
    Item::Type(TypeDecl {
        kind: TypeKind::Struct,
        name: format!("__Tuple{arity}"),
        is_public: true,
        is_opaque: false,
        type_params,
        malformed_generic_param_spans: Vec::new(),
        derives: vec!["Clone".to_string()],
        fields,
        malformed_field_spans: Vec::new(),
        drop_body: None,
        span,
    })
}

fn collect_item_arities(item: &Item, arities: &mut BTreeSet<usize>) {
    match item {
        Item::Function(function) => {
            for param in &function.params {
                collect_type_arities(&param.ty, arities);
                if let Some(default) = &param.default {
                    collect_expr_arities(default, arities);
                }
            }
            if let Some(return_ty) = &function.return_ty {
                collect_type_arities(return_ty, arities);
            }
            collect_block_arities(&function.body, arities);
        }
        Item::Const(decl) => {
            if let Some(annotation) = &decl.type_annotation {
                collect_type_arities(annotation, arities);
            }
            collect_expr_arities(&decl.value, arities);
        }
        Item::Type(decl) => {
            for field in &decl.fields {
                collect_type_arities(&field.ty, arities);
            }
            if let Some(body) = &decl.drop_body {
                collect_block_arities(body, arities);
            }
        }
        Item::SumType(decl) => {
            for variant in &decl.variants {
                for field in &variant.fields {
                    collect_type_arities(&field.ty, arities);
                }
            }
        }
        Item::TypeAlias(decl) => collect_type_arities(&decl.target, arities),
        Item::Module(_) | Item::Use(_) => {}
    }
}

fn collect_type_arities(ty: &TypeRef, arities: &mut BTreeSet<usize>) {
    if let Some(arity) = tuple_arity(&ty.name) {
        arities.insert(arity);
    }
    for arg in &ty.args {
        collect_type_arities(arg, arities);
    }
    for param in &ty.fn_params {
        collect_type_arities(param, arities);
    }
    if let Some(ret) = &ty.fn_return {
        collect_type_arities(ret, arities);
    }
}

fn collect_block_arities(block: &Block, arities: &mut BTreeSet<usize>) {
    for statement in &block.statements {
        collect_stmt_arities(statement, arities);
    }
}

fn collect_stmt_arities(statement: &Stmt, arities: &mut BTreeSet<usize>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(annotation) = &stmt.type_annotation {
                collect_type_arities(annotation, arities);
            }
            if let Some(value) = &stmt.value {
                collect_expr_arities(value, arities);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_arities(value, arities);
            }
        }
        Stmt::Expr(expr) => collect_expr_arities(expr, arities),
        Stmt::Assign(stmt) => {
            collect_expr_arities(&stmt.target, arities);
            collect_expr_arities(&stmt.value, arities);
        }
        Stmt::With(stmt) => {
            collect_expr_arities(&stmt.resource, arities);
            collect_block_arities(&stmt.body, arities);
        }
        Stmt::If(stmt) => {
            collect_expr_arities(&stmt.condition, arities);
            collect_block_arities(&stmt.then_body, arities);
            if let Some(else_body) = &stmt.else_body {
                collect_block_arities(else_body, arities);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_expr_arities(condition, arities);
            }
            collect_block_arities(&stmt.body, arities);
        }
        Stmt::For(stmt) => {
            collect_expr_arities(&stmt.iterable, arities);
            collect_block_arities(&stmt.body, arities);
        }
        Stmt::LetElse(stmt) => {
            collect_expr_arities(&stmt.value, arities);
            collect_block_arities(&stmt.else_body, arities);
        }
        Stmt::TaskGroup(stmt) => collect_block_arities(&stmt.body, arities),
        Stmt::Match(stmt) => {
            collect_expr_arities(&stmt.value, arities);
            for arm in &stmt.arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_arities(guard, arities);
                }
                collect_block_arities(&arm.body, arities);
            }
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_expr_arities(&arm.operation, arities);
                collect_block_arities(&arm.body, arities);
            }
        }
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

fn collect_expr_arities(expr: &Expr, arities: &mut BTreeSet<usize>) {
    match expr {
        Expr::Field { base, .. } => collect_expr_arities(base, arities),
        Expr::Index { base, index, .. } => {
            collect_expr_arities(base, arities);
            collect_expr_arities(index, arities);
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_arities(left, arities);
            collect_expr_arities(right, arities);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => collect_expr_arities(value, arities),
        Expr::Call { callee, args, .. } => {
            if let Callee::Name(name) = callee
                && let Some(arity) = tuple_arity(name)
            {
                arities.insert(arity);
            }
            if let Callee::ReceiverCall { receiver, .. } = callee {
                collect_expr_arities(receiver, arities);
            }
            for arg in args {
                collect_expr_arities(&arg.value, arities);
            }
        }
        Expr::Closure { body, .. } => collect_block_arities(body, arities),
        Expr::Match { value, arms, .. } => {
            collect_expr_arities(value, arities);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_arities(guard, arities);
                }
                collect_block_arities(&arm.body, arities);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_arities(&field.value, arities);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_arities(&entry.key, arities);
                collect_expr_arities(&entry.value, arities);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_arities(item, arities);
            }
        }
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => {}
    }
}
