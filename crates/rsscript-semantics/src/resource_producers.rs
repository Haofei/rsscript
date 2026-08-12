//! HIR resource-producer classification and boundary diagnostics.

use crate::{
    hir::{Hir, HirBlock, HirExpr, HirStmt, HirTypeKind},
    resource_producer_escape_diagnostic, resource_producer_missing_try_diagnostic,
};
use rsscript_diagnostics::{Diagnostic, Span};

/// The resolved shape of a resource-producing expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceProducerKind {
    Resource,
    ResultResource { ok_type: String },
}

/// Classify an expression which creates a resource or a `Result` whose `Ok`
/// value is a resource. This is a semantic HIR query; callers merely provide
/// the expression's lexical context.
pub fn resource_producer_kind(hir: &Hir, expr: &HirExpr) -> Option<ResourceProducerKind> {
    match expr {
        HirExpr::Call { .. } => {
            if expression_type_is_resource(hir, expr) {
                Some(ResourceProducerKind::Resource)
            } else {
                result_resource_ok_type(hir, expr)
                    .map(|ok_type| ResourceProducerKind::ResultResource { ok_type })
            }
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. }
            if expression_type_is_resource(hir, expr) =>
        {
            resource_producer_kind(hir, value)
        }
        _ => None,
    }
}

/// Report a resource producer used outside a resource-owning context.
pub fn resource_producer_context_diagnostic(
    hir: &Hir,
    expr: &HirExpr,
    allowed_resource_context: bool,
) -> Option<Diagnostic> {
    (!allowed_resource_context && resource_producer_kind(hir, expr).is_some()).then(|| {
        resource_producer_escape_diagnostic(
            hir_expr_type_name(expr).unwrap_or("resource"),
            hir_expr_span(expr).clone(),
        )
    })
}

/// Derive every resource-producer boundary diagnostic reachable from one
/// expression.  Traversal is semantic because it determines which nested
/// expressions remain in a resource-owning lexical position; callers only
/// select the enclosing expression and its initial context.
pub fn resource_producer_diagnostics(
    hir: &Hir,
    expr: &HirExpr,
    allowed_resource_context: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_resource_producer_expr_diagnostics(
        hir,
        expr,
        allowed_resource_context,
        &mut diagnostics,
    );
    diagnostics
}

/// Report the missing `?` at a `with` boundary for `Result<Resource, E>`.
pub fn result_resource_with_try_diagnostic(hir: &Hir, expr: &HirExpr) -> Option<Diagnostic> {
    if matches!(expr, HirExpr::Try { .. }) {
        return None;
    }
    let ResourceProducerKind::ResultResource { ok_type } = resource_producer_kind(hir, expr)?
    else {
        return None;
    };
    Some(resource_producer_missing_try_diagnostic(
        &ok_type,
        hir_expr_span(expr).clone(),
    ))
}

fn collect_resource_producer_expr_diagnostics(
    hir: &Hir,
    expr: &HirExpr,
    allowed_resource_context: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(diagnostic) =
        resource_producer_context_diagnostic(hir, expr, allowed_resource_context)
    {
        diagnostics.push(diagnostic);
        return;
    }
    if resource_producer_kind(hir, expr).is_some() {
        collect_resource_producer_children(hir, expr, diagnostics);
        return;
    }

    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_resource_producer_expr_diagnostics(hir, &arg.value, false, diagnostics);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_resource_producer_expr_diagnostics(
            hir,
            value,
            allowed_resource_context,
            diagnostics,
        ),
        HirExpr::Binary { left, right, .. } => {
            collect_resource_producer_expr_diagnostics(hir, left, false, diagnostics);
            collect_resource_producer_expr_diagnostics(hir, right, false, diagnostics);
        }
        HirExpr::Field { base, .. } => {
            collect_resource_producer_expr_diagnostics(hir, base, false, diagnostics);
        }
        HirExpr::Index { base, index, .. } => {
            collect_resource_producer_expr_diagnostics(hir, base, false, diagnostics);
            collect_resource_producer_expr_diagnostics(hir, index, false, diagnostics);
        }
        HirExpr::Closure { body, .. } => {
            collect_resource_producer_block_diagnostics(hir, body, diagnostics);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_resource_producer_expr_diagnostics(
                hir,
                value,
                allowed_resource_context,
                diagnostics,
            );
            for arm in arms {
                collect_resource_producer_block_diagnostics(hir, &arm.body, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_resource_producer_expr_diagnostics(hir, &entry.key, false, diagnostics);
                collect_resource_producer_expr_diagnostics(hir, &entry.value, false, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_resource_producer_children(
    hir: &Hir,
    expr: &HirExpr,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_resource_producer_expr_diagnostics(hir, &arg.value, false, diagnostics);
            }
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            collect_resource_producer_expr_diagnostics(hir, value, true, diagnostics);
        }
        _ => {}
    }
}

fn collect_resource_producer_block_diagnostics(
    hir: &Hir,
    block: &HirBlock,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        collect_resource_producer_stmt_diagnostics(hir, statement, diagnostics);
    }
}

fn collect_resource_producer_stmt_diagnostics(
    hir: &Hir,
    statement: &HirStmt,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_resource_producer_expr_diagnostics(hir, value, false, diagnostics);
        }
        HirStmt::With { resource, body, .. } => {
            if let Some(diagnostic) = result_resource_with_try_diagnostic(hir, resource) {
                diagnostics.push(diagnostic);
            }
            collect_resource_producer_expr_diagnostics(hir, resource, true, diagnostics);
            collect_resource_producer_block_diagnostics(hir, body, diagnostics);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_resource_producer_expr_diagnostics(hir, condition, false, diagnostics);
            collect_resource_producer_block_diagnostics(hir, then_body, diagnostics);
            if let Some(else_body) = else_body {
                collect_resource_producer_block_diagnostics(hir, else_body, diagnostics);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_resource_producer_expr_diagnostics(hir, condition, false, diagnostics);
            }
            collect_resource_producer_block_diagnostics(hir, body, diagnostics);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_resource_producer_expr_diagnostics(hir, iterable, false, diagnostics);
            collect_resource_producer_block_diagnostics(hir, body, diagnostics);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_resource_producer_expr_diagnostics(hir, value, false, diagnostics);
            for arm in arms {
                collect_resource_producer_block_diagnostics(hir, &arm.body, diagnostics);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_resource_producer_expr_diagnostics(hir, &arm.operation, false, diagnostics);
                collect_resource_producer_block_diagnostics(hir, &arm.body, diagnostics);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn expression_type_is_resource(hir: &Hir, expr: &HirExpr) -> bool {
    hir_expr_type_name(expr)
        .is_some_and(|type_name| hir.type_kind(type_name) == Some(HirTypeKind::Resource))
}

fn result_resource_ok_type(hir: &Hir, expr: &HirExpr) -> Option<String> {
    let type_name = hir_expr_type_name(expr)?;
    let inner = type_name.strip_prefix("Result<")?.strip_suffix('>')?;
    let ok_type = split_top_level_type_args(inner).first()?.to_string();
    (hir.type_kind(&ok_type) == Some(HirTypeKind::Resource)).then_some(ok_type)
}

fn split_top_level_type_args(value: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in value.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(value[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    args.push(value[start..].trim());
    args
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { type_name, .. }
        | HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Char { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_diagnostics::code;
    use rsscript_syntax::parse_source;

    #[test]
    fn derives_missing_try_at_a_result_resource_with_boundary() {
        let program = parse_source(
            "resource-producer.rss",
            r#"
resource File { fd: Int }
fn File.open(path: Path) -> Result<File, IOError>
fn main(path: Path) -> Unit {
    with File.open(path) as file {}
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let body = hir
            .function_body("main")
            .and_then(|body| body.block.as_ref())
            .unwrap();
        let HirStmt::With { resource, .. } = &body.statements[0] else {
            panic!("expected a with statement");
        };

        assert_eq!(
            result_resource_with_try_diagnostic(&hir, resource)
                .expect("result resource must require `?`")
                .code,
            code::RESOURCE_PRODUCER_MISSING_TRY
        );
        assert!(resource_producer_diagnostics(&hir, resource, true).is_empty());
    }

    #[test]
    fn traversal_reports_a_nested_resource_outside_with() {
        let program = parse_source(
            "resource-producer.rss",
            r#"
resource File { fd: Int }
fn File.open(path: Path) -> File
fn identity(value: read File) -> File
fn main(path: Path) -> Unit {
    identity(File.open(path))
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let body = hir
            .function_body("main")
            .and_then(|body| body.block.as_ref())
            .unwrap();
        let HirStmt::Expr(expr) = &body.statements[0] else {
            panic!("expected an expression statement");
        };

        assert_eq!(resource_producer_diagnostics(&hir, expr, false).len(), 1);
    }
}
