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

use std::collections::HashMap;

use super::ast::{Block, Callee, Expr, Item, Program, Stmt};

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
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => {}
    }
}
