use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::{Diagnostic, code};
use crate::syntax::ast::{Block, Callee, Expr, Item, Program, Stmt};
use crate::text_util::type_root_name;

use super::protocol_method_keys;
use crate::rust_lower::intrinsics::runtime_intrinsic_target;

pub(in crate::rust_lower) fn validate_executable_declarations(
    program: &Program,
    native_bindings: &BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    let mut implemented = BTreeSet::new();
    let mut bodyless = BTreeMap::new();
    let mut native_bodyless = BTreeMap::new();
    let protocol_method_keys = protocol_method_keys(program);
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let key = executable_declaration_function_key(&function.name);
        if protocol_method_keys.contains(&key) {
            continue;
        }
        if function.body.statements.is_empty() {
            if function.is_native {
                native_bodyless.insert(key, function.name.clone());
            } else {
                bodyless.insert(key, function.name.clone());
            }
        } else {
            implemented.insert(key);
        }
    }
    for key in &implemented {
        bodyless.remove(key);
        native_bodyless.remove(key);
    }
    if bodyless.is_empty() && native_bodyless.is_empty() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    let context = ExecutableDeclarationValidation {
        bodyless: &bodyless,
        native_bodyless: &native_bodyless,
        native_bindings,
    };
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if function.body.statements.is_empty() {
            continue;
        }
        validate_executable_declarations_in_block(&function.body, &context, &mut diagnostics);
    }
    diagnostics
}

struct ExecutableDeclarationValidation<'a> {
    bodyless: &'a BTreeMap<String, String>,
    native_bodyless: &'a BTreeMap<String, String>,
    native_bindings: &'a BTreeMap<String, String>,
}

fn validate_executable_declarations_in_block(
    block: &Block,
    context: &ExecutableDeclarationValidation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        validate_executable_declarations_in_stmt(statement, context, diagnostics);
    }
}

fn validate_executable_declarations_in_stmt(
    statement: &Stmt,
    context: &ExecutableDeclarationValidation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                validate_executable_declarations_in_expr(value, context, diagnostics);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                validate_executable_declarations_in_expr(value, context, diagnostics);
            }
        }
        Stmt::With(stmt) => {
            validate_executable_declarations_in_expr(&stmt.resource, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::If(stmt) => {
            validate_executable_declarations_in_expr(&stmt.condition, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.then_body, context, diagnostics);
            if let Some(else_body) = &stmt.else_body {
                validate_executable_declarations_in_block(else_body, context, diagnostics);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                validate_executable_declarations_in_expr(condition, context, diagnostics);
            }
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::For(stmt) => {
            validate_executable_declarations_in_expr(&stmt.iterable, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::TaskGroup(stmt) => {
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                validate_executable_declarations_in_expr(&arm.operation, context, diagnostics);
                validate_executable_declarations_in_block(&arm.body, context, diagnostics);
            }
        }
        Stmt::Match(stmt) => {
            validate_executable_declarations_in_expr(&stmt.value, context, diagnostics);
            for arm in &stmt.arms {
                validate_executable_declarations_in_block(&arm.body, context, diagnostics);
            }
        }
        Stmt::LetElse(stmt) => {
            validate_executable_declarations_in_expr(&stmt.value, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.else_body, context, diagnostics);
        }
        Stmt::Assign(stmt) => {
            validate_executable_declarations_in_expr(&stmt.target, context, diagnostics);
            validate_executable_declarations_in_expr(&stmt.value, context, diagnostics);
        }
        Stmt::Expr(expr) => validate_executable_declarations_in_expr(expr, context, diagnostics),
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

fn validate_executable_declarations_in_expr(
    expr: &Expr,
    context: &ExecutableDeclarationValidation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            let key = executable_declaration_callee_key(callee);
            if runtime_intrinsic_target(callee).is_none()
                && let Some(function_name) = context.bodyless.get(&key)
            {
                diagnostics.push(
                    Diagnostic::error(
                        code::UNSUPPORTED_SYNTAX,
                        "unsupported executable RSScript declaration call.",
                        span.clone(),
                        "unimplemented declaration call",
                    )
                    .with_cause(format!(
                        "`{function_name}` is a declaration without a RSScript body. Provide an implementation or bind it as a native/runtime intrinsic before executable lowering."
                    )),
                );
            }
            if runtime_intrinsic_target(callee).is_none()
                && !context.native_bindings.contains_key(&key)
                && let Some(function_name) = context.native_bodyless.get(&key)
            {
                diagnostics.push(
                    Diagnostic::error(
                        code::UNSUPPORTED_SYNTAX,
                        "unbound native RSScript declaration call.",
                        span.clone(),
                        "unbound native declaration call",
                    )
                    .with_cause(format!(
                        "`{function_name}` is a native declaration without a configured Rust binding. Add a native binding before executable lowering."
                    )),
                );
            }
            for arg in args {
                validate_executable_declarations_in_expr(&arg.value, context, diagnostics);
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_executable_declarations_in_expr(left, context, diagnostics);
            validate_executable_declarations_in_expr(right, context, diagnostics);
        }
        Expr::Field { base, .. } => {
            validate_executable_declarations_in_expr(base, context, diagnostics);
        }
        Expr::Index { base, index, .. } => {
            validate_executable_declarations_in_expr(base, context, diagnostics);
            validate_executable_declarations_in_expr(index, context, diagnostics);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            validate_executable_declarations_in_expr(value, context, diagnostics);
        }
        Expr::Closure { body, .. } => {
            validate_executable_declarations_in_block(body, context, diagnostics);
        }
        Expr::Match { value, arms, .. } => {
            validate_executable_declarations_in_expr(value, context, diagnostics);
            for arm in arms {
                validate_executable_declarations_in_block(&arm.body, context, diagnostics);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::MapLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(in crate::rust_lower) fn executable_declaration_function_key(name: &str) -> String {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        format!("{}.{}", type_root_name(namespace), name)
    } else {
        name.to_string()
    }
}

pub(in crate::rust_lower) fn executable_declaration_callee_key(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => type_root_name(name).to_string(),
        Callee::Qualified { namespace, name } => {
            format!("{}.{}", type_root_name(namespace), type_root_name(name))
        }
        Callee::ReceiverCall { method, .. } => type_root_name(method).to_string(),
    }
}
