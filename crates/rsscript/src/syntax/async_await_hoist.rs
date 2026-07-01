//! `await`-in-expression support via await-hoisting (A-normal form).
//!
//! The async lowering only handles `await` at a statement boundary (`let x =
//! await f()`, `return await f()`, a bare `await f()` statement) or inside an
//! `if`/`loop`/`match`/`with` body. A nested `await` — `f(x: await g())` — is
//! otherwise rejected (`RS0411`).
//!
//! This pass lifts each nested await out into a preceding
//! `let __rss_await_N = await <op>`, producing the linear awaits the backends
//! already lower identically. It is sound and contained: the output uses only
//! already-supported constructs, so the async lowering itself is untouched.
//!
//! Conservative boundaries (left as-is, so they stay rejected rather than risk
//! wrong evaluation order): the RHS of short-circuit `&&`/`||`, `match`/`if`
//! arms reached through an expression, and closure bodies.

use super::ast::{BinaryOp, Block, Callee, Expr, Item, LetKind, LetStmt, Program, Stmt};

/// Hoist nested awaits in every `async fn` body to statement boundaries.
pub fn hoist_async_awaits(program: &mut Program) {
    for item in &mut program.items {
        if let Item::Function(function) = item
            && function.is_async
        {
            let mut counter = 0usize;
            hoist_block(&mut function.body, &mut counter);
        }
    }
}

fn hoist_block(block: &mut Block, counter: &mut usize) {
    let mut rewritten: Vec<Stmt> = Vec::with_capacity(block.statements.len());
    for mut statement in std::mem::take(&mut block.statements) {
        let mut hoisted: Vec<Stmt> = Vec::new();
        hoist_stmt(&mut statement, &mut hoisted, counter);
        rewritten.extend(hoisted);
        rewritten.push(statement);
    }
    block.statements = rewritten;
}

fn hoist_stmt(stmt: &mut Stmt, hoisted: &mut Vec<Stmt>, counter: &mut usize) {
    match stmt {
        Stmt::Let(let_stmt) => {
            if let Some(value) = &mut let_stmt.value {
                hoist_value(value, hoisted, counter);
            }
        }
        Stmt::Return(ret) => {
            if let Some(value) = &mut ret.value {
                hoist_value(value, hoisted, counter);
            }
        }
        Stmt::Expr(value) => hoist_value(value, hoisted, counter),
        Stmt::Assign(assign) => {
            // The target is an evaluated place: any await in a field/index base or
            // index expression is nested and must be hoisted.
            hoist_nested(&mut assign.target, hoisted, counter);
            hoist_value(&mut assign.value, hoisted, counter);
        }
        // Control-flow statements own their own blocks (boundaries). Recurse into
        // the nested blocks; leave the condition/scrutinee to the existing async
        // boundary lowering.
        Stmt::If(if_stmt) => {
            hoist_block(&mut if_stmt.then_body, counter);
            if let Some(else_body) = &mut if_stmt.else_body {
                hoist_block(else_body, counter);
            }
        }
        Stmt::Loop(loop_stmt) => hoist_block(&mut loop_stmt.body, counter),
        Stmt::For(for_stmt) => hoist_block(&mut for_stmt.body, counter),
        Stmt::With(with_stmt) => hoist_block(&mut with_stmt.body, counter),
        Stmt::TaskGroup(task_group) => hoist_block(&mut task_group.body, counter),
        Stmt::Match(match_stmt) => {
            for arm in &mut match_stmt.arms {
                hoist_block(&mut arm.body, counter);
            }
        }
        Stmt::Select(select) => {
            for arm in &mut select.arms {
                hoist_block(&mut arm.body, counter);
            }
        }
        Stmt::LetElse(let_else) => {
            hoist_value(&mut let_else.value, hoisted, counter);
            hoist_block(&mut let_else.else_body, counter);
        }
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

/// A root value position (`let`/`return`/`expr`/assign RHS): a top-level `await`
/// (through transparent effect/`?` wrappers) is already linear, so only its
/// operand is searched for nested awaits.
fn hoist_value(expr: &mut Expr, hoisted: &mut Vec<Stmt>, counter: &mut usize) {
    match expr {
        Expr::Effect { value, .. } => hoist_value(value, hoisted, counter),
        Expr::Await { value, .. } => hoist_nested(value, hoisted, counter),
        Expr::Try { value, .. } if matches!(value.as_ref(), Expr::Await { .. }) => {
            if let Expr::Await { value: inner, .. } = value.as_mut() {
                hoist_nested(inner, hoisted, counter);
            }
        }
        _ => hoist_nested(expr, hoisted, counter),
    }
}

/// A nested (non-root) position: every `await` found here is lifted to a
/// preceding `let __rss_await_N = await <op>` and replaced with the temp.
fn hoist_nested(expr: &mut Expr, hoisted: &mut Vec<Stmt>, counter: &mut usize) {
    if is_await_unit(expr) {
        // Hoist awaits inside the awaited operand first (inner-to-outer order).
        if let Some(inner) = await_unit_operand_mut(expr) {
            hoist_nested(inner, hoisted, counter);
        }
        let span = expr.span().clone();
        let name = format!("__rss_await_{counter}");
        *counter += 1;
        let original = std::mem::replace(expr, Expr::Ident(name.clone(), span.clone()));
        hoisted.push(Stmt::Let(LetStmt {
            kind: LetKind::Managed,
            name,
            type_annotation: None,
            value: Some(original),
            is_async: false,
            is_mut: false,
            destructure: None,
            malformed: false,
            span,
        }));
        return;
    }
    match expr {
        Expr::Call { callee, args, .. } => {
            if let Callee::ReceiverCall { receiver, .. } = callee {
                hoist_nested(receiver, hoisted, counter);
            }
            for arg in args.iter_mut() {
                hoist_nested(&mut arg.value, hoisted, counter);
            }
        }
        // Short-circuit operators: the right operand is conditionally evaluated,
        // so hoisting it would change semantics — leave it (stays rejected).
        Expr::Binary {
            op: BinaryOp::LogicalAnd | BinaryOp::LogicalOr,
            left,
            ..
        } => hoist_nested(left, hoisted, counter),
        Expr::Binary { left, right, .. } => {
            hoist_nested(left, hoisted, counter);
            hoist_nested(right, hoisted, counter);
        }
        Expr::Field { base, .. } => hoist_nested(base, hoisted, counter),
        Expr::Index { base, index, .. } => {
            hoist_nested(base, hoisted, counter);
            hoist_nested(index, hoisted, counter);
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            hoist_nested(value, hoisted, counter);
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                hoist_nested(&mut field.value, hoisted, counter);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                hoist_nested(&mut entry.key, hoisted, counter);
                hoist_nested(&mut entry.value, hoisted, counter);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                hoist_nested(item, hoisted, counter);
            }
        }
        // Match-expression arms are statement-boundary scopes of their own; hoist
        // within each arm body. Leave the scrutinee and closures to existing rules.
        Expr::Match { arms, .. } => {
            for arm in arms {
                hoist_block(&mut arm.body, counter);
            }
        }
        Expr::Closure { .. }
        | Expr::Spawn { .. }
        | Expr::Await { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

/// `await op` or `await op?` (`Try` wrapping an `Await`) — the unit lifted whole.
fn is_await_unit(expr: &Expr) -> bool {
    matches!(expr, Expr::Await { .. })
        || matches!(expr, Expr::Try { value, .. } if matches!(value.as_ref(), Expr::Await { .. }))
}

/// The awaited operand inside an await-unit, for hoisting its own nested awaits.
fn await_unit_operand_mut(expr: &mut Expr) -> Option<&mut Expr> {
    match expr {
        Expr::Await { value, .. } => Some(value),
        Expr::Try { value, .. } => match value.as_mut() {
            Expr::Await { value: inner, .. } => Some(inner),
            _ => None,
        },
        _ => None,
    }
}
