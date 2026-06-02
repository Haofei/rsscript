//! A file-local symbol index for editor navigation (go-to-definition,
//! find-references, document symbols).
//!
//! It is built from the public syntax tree ([`crate::syntax::parse_source`]),
//! which already carries spans on every node. Resolution is **name-based and
//! file-local**: it does not model scopes or cross-file imports, so a `let x`
//! and a parameter `x` in different functions are treated as the same symbol.
//! That is intentionally simple and good enough for navigation; scope-accurate
//! resolution would reuse the analyzer's resolver and is a future refinement.

use crate::diagnostic::Span;
use crate::syntax::ast::{
    Block, Callee, ConstDecl, Expr, FunctionDecl, Item, MatchPattern, Program, Stmt, SumTypeDecl,
    TypeAliasDecl, TypeDecl, TypeRef,
};
use crate::syntax::parse_source;

/// What a definition is, kept for document symbols and to disambiguate a few
/// name collisions (a type reference prefers a type definition).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Type,
    Const,
    Param,
    Local,
}

/// A named declaration and where it lives.
#[derive(Debug, Clone)]
pub struct Definition {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
}

/// A use of a name, with the precise span of the referencing token(s).
#[derive(Debug, Clone)]
pub struct Reference {
    pub name: String,
    pub span: Span,
    pub is_type: bool,
}

/// File-local index of definitions and references.
pub struct SymbolIndex {
    definitions: Vec<Definition>,
    references: Vec<Reference>,
}

/// Parse `source` and build its [`SymbolIndex`].
pub fn symbol_index(file: &str, source: &str) -> SymbolIndex {
    let program = parse_source(file, source);
    let mut builder = Builder {
        definitions: Vec::new(),
        references: Vec::new(),
    };
    builder.visit_program(&program);
    SymbolIndex {
        definitions: builder.definitions,
        references: builder.references,
    }
}

impl SymbolIndex {
    pub fn definitions(&self) -> &[Definition] {
        &self.definitions
    }

    pub fn references(&self) -> &[Reference] {
        &self.references
    }

    /// Span to jump to for the symbol at 1-based `line` / `column` (counted in
    /// `char`s), or `None` if nothing there resolves to a local definition.
    pub fn definition_at(&self, line: usize, column: usize) -> Option<&Span> {
        let cursor = self.cursor_at(line, column)?;
        self.lookup_definition(&cursor.name, cursor.is_type)
            .map(|definition| &definition.span)
    }

    /// All occurrences of the symbol at `line` / `column`. Includes the
    /// definition itself when `include_declaration` is set.
    pub fn references_at(
        &self,
        line: usize,
        column: usize,
        include_declaration: bool,
    ) -> Vec<Span> {
        let Some(cursor) = self.cursor_at(line, column) else {
            return Vec::new();
        };
        let mut spans = Vec::new();
        if include_declaration {
            for definition in &self.definitions {
                if definition.name == cursor.name {
                    spans.push(definition.span.clone());
                }
            }
        }
        for reference in &self.references {
            if reference.name == cursor.name {
                spans.push(reference.span.clone());
            }
        }
        spans
    }

    fn lookup_definition(&self, name: &str, prefer_type: bool) -> Option<&Definition> {
        if prefer_type {
            if let Some(definition) = self
                .definitions
                .iter()
                .find(|definition| definition.name == name && definition.kind == SymbolKind::Type)
            {
                return Some(definition);
            }
        }
        self.definitions
            .iter()
            .find(|definition| definition.name == name)
    }

    /// The most specific (smallest-span) definition or reference covering the
    /// point. Smallest-span-wins resolves nesting, e.g. a parameter token that
    /// sits inside its function's signature span.
    fn cursor_at(&self, line: usize, column: usize) -> Option<Cursor> {
        let mut best: Option<(usize, Cursor)> = None;
        let mut consider = |span: &Span, name: &str, is_type: bool| {
            if span_contains(span, line, column) {
                let better = match &best {
                    Some((length, _)) => span.length < *length,
                    None => true,
                };
                if better {
                    best = Some((
                        span.length,
                        Cursor {
                            name: name.to_string(),
                            is_type,
                        },
                    ));
                }
            }
        };
        for reference in &self.references {
            consider(&reference.span, &reference.name, reference.is_type);
        }
        for definition in &self.definitions {
            consider(
                &definition.span,
                &definition.name,
                definition.kind == SymbolKind::Type,
            );
        }
        best.map(|(_, cursor)| cursor)
    }
}

struct Cursor {
    name: String,
    is_type: bool,
}

fn span_contains(span: &Span, line: usize, column: usize) -> bool {
    span.line == line && column >= span.column && column < span.column + span.length.max(1)
}

/// A span with the same start as `span` but a fixed length — used to carve the
/// precise callee/receiver out of a call expression's full span.
fn sub_span(span: &Span, length: usize) -> Span {
    Span {
        file: span.file.clone(),
        line: span.line,
        column: span.column,
        length,
    }
}

struct Builder {
    definitions: Vec<Definition>,
    references: Vec<Reference>,
}

impl Builder {
    fn define(&mut self, name: &str, kind: SymbolKind, span: &Span) {
        if name.is_empty() {
            return;
        }
        self.definitions.push(Definition {
            name: name.to_string(),
            kind,
            span: span.clone(),
        });
    }

    fn reference(&mut self, name: &str, span: Span, is_type: bool) {
        if name.is_empty() {
            return;
        }
        self.references.push(Reference {
            name: name.to_string(),
            span,
            is_type,
        });
    }

    fn visit_program(&mut self, program: &Program) {
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn visit_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => self.visit_function(function),
            Item::Type(decl) => self.visit_type_decl(decl),
            Item::SumType(decl) => self.visit_sum_type(decl),
            Item::TypeAlias(decl) => self.visit_type_alias(decl),
            Item::Const(decl) => self.visit_const(decl),
            Item::Module(_) | Item::Use(_) => {}
        }
    }

    fn visit_function(&mut self, function: &FunctionDecl) {
        self.define(&function.name, SymbolKind::Function, &function.span);
        for param in &function.params {
            self.define(&param.name, SymbolKind::Param, &param.span);
            self.visit_type_ref(&param.ty);
        }
        if let Some(return_ty) = &function.return_ty {
            self.visit_type_ref(return_ty);
        }
        self.visit_block(&function.body);
    }

    fn visit_type_decl(&mut self, decl: &TypeDecl) {
        self.define(&decl.name, SymbolKind::Type, &decl.span);
        for field in &decl.fields {
            self.visit_type_ref(&field.ty);
        }
        if let Some(body) = &decl.drop_body {
            self.visit_block(body);
        }
    }

    fn visit_sum_type(&mut self, decl: &SumTypeDecl) {
        self.define(&decl.name, SymbolKind::Type, &decl.span);
        for variant in &decl.variants {
            for field in &variant.fields {
                self.visit_type_ref(&field.ty);
            }
        }
    }

    fn visit_type_alias(&mut self, decl: &TypeAliasDecl) {
        self.define(&decl.name, SymbolKind::Type, &decl.span);
        self.visit_type_ref(&decl.target);
    }

    fn visit_const(&mut self, decl: &ConstDecl) {
        self.define(&decl.name, SymbolKind::Const, &decl.span);
        if let Some(type_annotation) = &decl.type_annotation {
            self.visit_type_ref(type_annotation);
        }
        self.visit_expr(&decl.value);
    }

    fn visit_type_ref(&mut self, ty: &TypeRef) {
        self.reference(&ty.name, ty.span.clone(), true);
        for arg in &ty.args {
            self.visit_type_ref(arg);
        }
        for param in &ty.fn_params {
            self.visit_type_ref(param);
        }
        if let Some(ret) = &ty.fn_return {
            self.visit_type_ref(ret);
        }
    }

    fn visit_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.visit_stmt(statement);
        }
    }

    fn visit_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(let_stmt) => {
                self.define(&let_stmt.name, SymbolKind::Local, &let_stmt.span);
                if let Some(type_annotation) = &let_stmt.type_annotation {
                    self.visit_type_ref(type_annotation);
                }
                if let Some(value) = &let_stmt.value {
                    self.visit_expr(value);
                }
            }
            Stmt::Return(return_stmt) => {
                if let Some(value) = &return_stmt.value {
                    self.visit_expr(value);
                }
            }
            Stmt::With(with_stmt) => {
                self.visit_expr(&with_stmt.resource);
                self.visit_block(&with_stmt.body);
            }
            Stmt::If(if_stmt) => {
                self.visit_expr(&if_stmt.condition);
                self.visit_block(&if_stmt.then_body);
                if let Some(else_body) = &if_stmt.else_body {
                    self.visit_block(else_body);
                }
            }
            Stmt::Loop(loop_stmt) => {
                if let Some(condition) = &loop_stmt.condition {
                    self.visit_expr(condition);
                }
                self.visit_block(&loop_stmt.body);
            }
            Stmt::For(for_stmt) => {
                self.visit_expr(&for_stmt.iterable);
                self.visit_block(&for_stmt.body);
            }
            Stmt::Match(match_stmt) => {
                self.visit_expr(&match_stmt.value);
                for arm in &match_stmt.arms {
                    self.visit_pattern(&arm.pattern);
                    self.visit_block(&arm.body);
                }
            }
            Stmt::TaskGroup(task_group) => self.visit_block(&task_group.body),
            Stmt::Select(select) => {
                for arm in &select.arms {
                    self.visit_expr(&arm.operation);
                    self.visit_block(&arm.body);
                }
            }
            Stmt::LetElse(let_else) => {
                self.visit_expr(&let_else.value);
                self.visit_block(&let_else.else_body);
            }
            Stmt::Assign(assign) => {
                self.visit_expr(&assign.target);
                self.visit_expr(&assign.value);
            }
            Stmt::Expr(expr) => self.visit_expr(expr),
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_) => {}
        }
    }

    fn visit_pattern(&mut self, pattern: &MatchPattern) {
        if let MatchPattern::Variant {
            binding: Some(binding),
            span,
            ..
        } = pattern
        {
            self.define(binding, SymbolKind::Local, span);
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(name, span) => self.reference(name, span.clone(), false),
            Expr::Call { callee, args, span } => {
                self.visit_callee(callee, span);
                for arg in args {
                    self.visit_expr(&arg.value);
                }
            }
            Expr::Field { base, .. } => self.visit_expr(base),
            Expr::Index { base, index, .. } => {
                self.visit_expr(base);
                self.visit_expr(index);
            }
            Expr::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => self.visit_expr(value),
            Expr::ArrayLiteral { items, .. } => {
                for item in items {
                    self.visit_expr(item);
                }
            }
            Expr::ObjectLiteral { fields, .. } => {
                for field in fields {
                    self.visit_expr(&field.value);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.visit_expr(&entry.key);
                    self.visit_expr(&entry.value);
                }
            }
            Expr::Match { value, arms, .. } => {
                self.visit_expr(value);
                for arm in arms {
                    self.visit_pattern(&arm.pattern);
                    self.visit_block(&arm.body);
                }
            }
            Expr::Closure { body, .. } => self.visit_block(body),
            Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    fn visit_callee(&mut self, callee: &Callee, call_span: &Span) {
        match callee {
            Callee::Name(name) => {
                self.reference(name, sub_span(call_span, name.chars().count()), false);
            }
            Callee::Qualified { namespace, name } => {
                let namespace_len = namespace.chars().count();
                // `Namespace` resolves to a type, `Namespace.method` to a function.
                self.reference(namespace, sub_span(call_span, namespace_len), true);
                let qualified = format!("{namespace}.{name}");
                let qualified_len = qualified.chars().count();
                self.reference(&qualified, sub_span(call_span, qualified_len), false);
            }
            Callee::ReceiverCall { receiver, .. } => self.visit_expr(receiver),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = concat!(
        "fn helper(value: read Int) -> Int {\n",
        "    return value\n",
        "}\n",
        "\n",
        "fn main() -> Int {\n",
        "    let total = helper(value: read 1)\n",
        "    return total\n",
        "}\n",
    );

    #[test]
    fn jumps_from_call_to_function_definition() {
        let index = symbol_index("t.rss", SOURCE);
        // `helper` call is on line 6 (1-based), starting at column 17.
        let def = index.definition_at(6, 18).expect("resolves helper");
        assert_eq!(def.line, 1); // `fn helper` declaration
    }

    #[test]
    fn jumps_from_use_to_let_binding() {
        let index = symbol_index("t.rss", SOURCE);
        // `total` use is on line 7 `    return total`, column 12.
        let def = index.definition_at(7, 12).expect("resolves total");
        assert_eq!(def.line, 6); // the `let total` binding
    }

    #[test]
    fn finds_all_references_of_a_parameter() {
        let index = symbol_index("t.rss", SOURCE);
        // `value` parameter usage in `return value` (line 2).
        let spans = index.references_at(2, 12, false);
        assert!(spans.iter().any(|span| span.line == 2));
    }
}
