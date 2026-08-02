use crate::diagnostic::Span;
use crate::syntax::ast::{Block, Expr, Stmt};

use super::helpers::stmt_span;
use super::lowerer::match_pattern_span;
use super::types::RustSourceMapEntry;

pub fn parse_source_map_json(source_map_json: &str) -> Result<Vec<RustSourceMapEntry>, String> {
    serde_json::from_str(source_map_json)
        .map_err(|error| format!("failed to parse RSScript source map JSON: {error}"))
}

pub(super) fn push_source_marker(
    out: &mut String,
    indent: usize,
    kind: &str,
    span: &Span,
) -> RustSourceMapEntry {
    let marker = format!(
        "{}// rss:span kind={kind} file={} line={} column={} length={}\n",
        "    ".repeat(indent),
        source_marker_value(&span.file),
        span.line,
        span.column,
        span.length
    );
    let generated = generated_span_at_end(out, "src/lib.rs", &marker);
    out.push_str(&marker);
    RustSourceMapEntry {
        kind: kind.to_string(),
        source: span.clone(),
        generated,
        ..Default::default()
    }
}

pub(super) fn generated_span_at_end(out: &str, file: &str, text: &str) -> Span {
    let (line, column) = generated_position(out);
    Span {
        file: file.to_string(),
        line,
        column,
        length: text.trim_end_matches('\n').chars().count().max(1),
    }
}

fn generated_position(out: &str) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for ch in out.chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn source_marker_value(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\n' | '\r' | '\t' => ['_'].into_iter().collect::<Vec<_>>(),
            _ => [character].into_iter().collect(),
        })
        .collect()
}

use super::*;

impl RustLowerer<'_> {
    pub(super) fn record_source_marker(
        &mut self,
        out: &mut String,
        indent: usize,
        kind: &str,
        span: &Span,
    ) -> RustSourceMapEntry {
        let entry = push_source_marker(out, indent, kind, span);
        self.source_map.push(entry.clone());
        entry
    }

    pub(super) fn widen_generated_span(&mut self, generated: &Span, line_count: usize) {
        for entry in &mut self.source_map {
            if entry.generated.file == generated.file
                && entry.generated.line == generated.line
                && entry.generated.column == generated.column
            {
                entry.generated.length = entry.generated.length.max(line_count.max(1));
            }
        }
    }

    pub(super) fn record_statement_source_map(&mut self, statement: &Stmt, generated: &Span) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.record_expr_source_map(value, generated);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.record_expr_source_map(value, generated);
                }
            }
            Stmt::With(stmt) => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "with".to_string(),
                    source: stmt.span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(&stmt.resource, generated);
                self.record_block_source_map(&stmt.body, generated);
            }
            Stmt::If(stmt) => {
                self.record_expr_source_map(&stmt.condition, generated);
                self.record_block_source_map(&stmt.then_body, generated);
                if let Some(else_body) = &stmt.else_body {
                    self.record_block_source_map(else_body, generated);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.record_expr_source_map(condition, generated);
                }
                self.record_block_source_map(&stmt.body, generated);
            }
            Stmt::For(stmt) => {
                self.record_expr_source_map(&stmt.iterable, generated);
                self.record_block_source_map(&stmt.body, generated);
            }
            Stmt::TaskGroup(stmt) => {
                self.record_block_source_map(&stmt.body, generated);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.record_expr_source_map(&arm.operation, generated);
                    self.record_block_source_map(&arm.body, generated);
                }
            }
            Stmt::Match(stmt) => {
                self.record_expr_source_map(&stmt.value, generated);
                for arm in &stmt.arms {
                    self.record_block_source_map(&arm.body, generated);
                }
            }
            Stmt::LetElse(stmt) => {
                self.record_expr_source_map(&stmt.value, generated);
                self.record_block_source_map(&stmt.else_body, generated);
            }
            Stmt::Assign(stmt) => {
                self.record_expr_source_map(&stmt.target, generated);
                self.record_expr_source_map(&stmt.value, generated);
            }
            Stmt::Expr(expr) => self.record_expr_source_map(expr, generated),
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

    pub(super) fn record_block_source_map(&mut self, block: &Block, generated: &Span) {
        for statement in &block.statements {
            self.source_map.push(RustSourceMapEntry {
                kind: "statement".to_string(),
                source: stmt_span(statement).clone(),
                generated: generated.clone(),
                ..Default::default()
            });
            self.record_statement_source_map(statement, generated);
        }
    }

    pub(super) fn record_expr_source_map(&mut self, expr: &Expr, generated: &Span) {
        match expr {
            Expr::Binary {
                left, right, span, ..
            } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "binary".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(left, generated);
                self.record_expr_source_map(right, generated);
            }
            Expr::Field { base, span, .. } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "field_path".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(base, generated);
            }
            Expr::Index { base, index, .. } => {
                self.record_expr_source_map(base, generated);
                self.record_expr_source_map(index, generated);
            }
            Expr::Call { callee, args, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "call".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                if self.is_external_boundary_call(callee) {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "native_call".to_string(),
                        source: span.clone(),
                        generated: generated.clone(),
                        ..Default::default()
                    });
                }
                for arg in args {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "named_arg".to_string(),
                        source: arg.span.clone(),
                        generated: generated.clone(),
                        ..Default::default()
                    });
                    self.record_expr_source_map(&arg.value, generated);
                }
            }
            Expr::Effect { value, .. } => self.record_expr_source_map(value, generated),
            Expr::Manage { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "manage".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Spawn { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "spawn".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Await { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "await".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Try { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "try".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Closure { body, span, .. } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "closure".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                self.record_block_source_map(body, generated);
            }
            Expr::Match { value, arms, .. } => {
                self.record_expr_source_map(value, generated);
                for arm in arms {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "match_pattern".to_string(),
                        source: match_pattern_span(&arm.pattern),
                        generated: generated.clone(),
                        ..Default::default()
                    });
                    self.record_block_source_map(&arm.body, generated);
                }
            }
            Expr::ObjectLiteral { fields, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "object_literal".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                for field in fields {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "object_literal_field".to_string(),
                        source: field.span.clone(),
                        generated: generated.clone(),
                        ..Default::default()
                    });
                    self.record_expr_source_map(&field.value, generated);
                }
            }
            Expr::MapLiteral { entries, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "map_literal".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                for entry in entries {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "map_literal_entry".to_string(),
                        source: entry.span.clone(),
                        generated: generated.clone(),
                        ..Default::default()
                    });
                    self.record_expr_source_map(&entry.key, generated);
                    self.record_expr_source_map(&entry.value, generated);
                }
            }
            Expr::ArrayLiteral { items, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "array_literal".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                });
                for item in items {
                    self.record_expr_source_map(item, generated);
                }
            }
            Expr::Ident(_, span) => self.source_map.push(RustSourceMapEntry {
                kind: "ident".to_string(),
                source: span.clone(),
                generated: generated.clone(),
                ..Default::default()
            }),
            Expr::Number(_, span) => self.source_map.push(RustSourceMapEntry {
                kind: "number".to_string(),
                source: span.clone(),
                generated: generated.clone(),
                ..Default::default()
            }),
            Expr::String(_, span) | Expr::CharLiteral(_, span) | Expr::MultilineString(_, span) => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "string".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                    ..Default::default()
                })
            }
            Expr::Unknown(_) => {}
        }
    }
}
