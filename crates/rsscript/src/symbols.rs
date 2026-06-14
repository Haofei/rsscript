//! A file-local symbol index for editor navigation (go-to-definition,
//! find-references, document symbols).
//!
//! It is built from the public syntax tree ([`crate::syntax::parse_source`]),
//! which already carries spans on every node. Resolution is file-local and
//! scope-aware for top-level symbols, function parameters, block locals, and
//! match bindings. It does not model cross-file imports yet; that belongs in a
//! package/workspace index layered above this one.

use std::collections::HashMap;

use crate::diagnostic::Span;
use crate::syntax::ast::{
    Block, Callee, ConstDecl, Expr, FieldDecl, FunctionDecl, Item, MatchPattern, Param, Program,
    Stmt, SumTypeDecl, TypeAliasDecl, TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source_raw;

/// What a definition is, kept for document symbols and to disambiguate a few
/// name collisions (a type reference prefers a type definition).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Type,
    Const,
    Param,
    Local,
    Field,
    Variant,
}

/// A named declaration and where it lives.
#[derive(Debug, Clone)]
pub struct Definition {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub detail: Option<String>,
}

/// A use of a name, with the precise span of the referencing token(s).
#[derive(Debug, Clone)]
pub struct Reference {
    pub name: String,
    pub span: Span,
    pub is_type: bool,
    pub definition: Option<usize>,
}

/// Symbol under a cursor, resolved to the best local definition when possible.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub detail: Option<String>,
}

/// Lookup key for the symbol under a cursor.
#[derive(Debug, Clone)]
pub struct SymbolLookup {
    pub name: String,
    pub is_type: bool,
    pub span: Span,
    pub local_definition: Option<usize>,
}

/// Hierarchical symbol for editor outline/document-symbol views.
#[derive(Debug, Clone)]
pub struct RssDocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: Option<String>,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<RssDocumentSymbol>,
}

/// File-local index of definitions and references.
pub struct SymbolIndex {
    definitions: Vec<Definition>,
    references: Vec<Reference>,
}

/// Parse `source` and build its [`SymbolIndex`].
pub fn symbol_index(file: &str, source: &str) -> SymbolIndex {
    let program = parse_source_raw(file, source);
    let mut builder = Builder {
        source,
        definitions: Vec::new(),
        references: Vec::new(),
        scopes: vec![HashMap::new()],
    };
    builder.visit_program(&program);
    SymbolIndex {
        definitions: builder.definitions,
        references: builder.references,
    }
}

/// One entry in the source-qualified symbol inventory: a declaration's module,
/// source-qualified name, kind, span, and the Rust symbol it lowers to. This is
/// the "stable source-qualified symbol identity" the port tooling (portman)
/// needs to map `helpers.rss::count` to its backend symbol without guessing.
#[derive(Debug, Clone)]
pub struct SymbolInventoryEntry {
    pub module: String,
    pub qualname: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub lowered_name: String,
}

/// Build the source-qualified symbol inventory for `file`: every item-level
/// declaration (functions incl. `Type.method`, types, consts, sum variants) with
/// its module path and lowered Rust name.
pub fn symbol_inventory(file: &str, source: &str) -> Vec<SymbolInventoryEntry> {
    let module = std::path::Path::new(file)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file)
        .to_string();
    let index = symbol_index(file, source);
    index
        .definitions()
        .iter()
        .filter(|definition| {
            matches!(
                definition.kind,
                SymbolKind::Function | SymbolKind::Type | SymbolKind::Const | SymbolKind::Variant
            )
        })
        .map(|definition| SymbolInventoryEntry {
            module: module.clone(),
            qualname: definition.name.clone(),
            kind: definition.kind,
            span: definition.span.clone(),
            lowered_name: crate::rust_lower::lowered_symbol_name(&definition.name),
        })
        .collect()
}

/// Parse `source` and return a top-level document outline.
pub fn document_symbols(file: &str, source: &str) -> Vec<RssDocumentSymbol> {
    let program = parse_source_raw(file, source);
    program
        .items
        .iter()
        .filter_map(document_symbol_for_item)
        .collect()
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
        self.lookup_definition(&cursor)
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
        let definition = self.lookup_definition(&cursor);
        if include_declaration && let Some(definition) = definition {
            spans.push(definition.span.clone());
        }
        for reference in &self.references {
            let same_symbol = match (cursor.definition, reference.definition) {
                (Some(left), Some(right)) => left == right,
                _ => reference.definition.is_none() && reference.name == cursor.name,
            };
            if same_symbol {
                spans.push(reference.span.clone());
            }
        }
        spans
    }

    /// Raw lookup key at 1-based `line` / `column`.
    pub fn lookup_at(&self, line: usize, column: usize) -> Option<SymbolLookup> {
        let cursor = self.cursor_at(line, column)?;
        Some(SymbolLookup {
            name: cursor.name,
            is_type: cursor.is_type,
            span: cursor.span,
            local_definition: cursor.definition,
        })
    }

    /// Symbol metadata at 1-based `line` / `column`, preferring the resolved
    /// local definition kind when a reference resolves.
    pub fn symbol_at(&self, line: usize, column: usize) -> Option<SymbolInfo> {
        let cursor = self.cursor_at(line, column)?;
        if let Some(definition) = self.lookup_definition(&cursor) {
            return Some(SymbolInfo {
                name: definition.name.clone(),
                kind: definition.kind,
                span: definition.span.clone(),
                detail: definition.detail.clone(),
            });
        }
        Some(SymbolInfo {
            name: cursor.name,
            kind: if cursor.is_type {
                SymbolKind::Type
            } else {
                SymbolKind::Local
            },
            span: cursor.span,
            detail: None,
        })
    }

    fn lookup_definition(&self, cursor: &Cursor) -> Option<&Definition> {
        if let Some(definition) = cursor.definition {
            return self.definitions.get(definition);
        }
        if cursor.is_type {
            if let Some(definition) = self.definitions.iter().find(|definition| {
                definition.name == cursor.name && definition.kind == SymbolKind::Type
            }) {
                return Some(definition);
            }
        }
        self.definitions
            .iter()
            .find(|definition| definition.name == cursor.name)
    }

    /// The most specific (smallest-span) definition or reference covering the
    /// point. Smallest-span-wins resolves nesting, e.g. a parameter token that
    /// sits inside its function's signature span.
    fn cursor_at(&self, line: usize, column: usize) -> Option<Cursor> {
        let mut best: Option<(usize, Cursor)> = None;
        let mut consider = |span: &Span, name: &str, is_type: bool, definition: Option<usize>| {
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
                            span: span.clone(),
                            definition,
                        },
                    ));
                }
            }
        };
        for reference in &self.references {
            consider(
                &reference.span,
                &reference.name,
                reference.is_type,
                reference.definition,
            );
        }
        for (index, definition) in self.definitions.iter().enumerate() {
            consider(
                &definition.span,
                &definition.name,
                definition.kind == SymbolKind::Type,
                Some(index),
            );
        }
        best.map(|(_, cursor)| cursor)
    }
}

struct Cursor {
    name: String,
    is_type: bool,
    span: Span,
    definition: Option<usize>,
}

fn document_symbol_for_item(item: &Item) -> Option<RssDocumentSymbol> {
    match item {
        Item::Function(function) => Some(RssDocumentSymbol {
            name: function.name.clone(),
            kind: SymbolKind::Function,
            detail: Some(function_detail(function)),
            span: function.span.clone(),
            selection_span: function.span.clone(),
            children: function
                .params
                .iter()
                .map(|param| RssDocumentSymbol {
                    name: param.name.clone(),
                    kind: SymbolKind::Param,
                    detail: Some(type_ref_display(&param.ty)),
                    span: param.span.clone(),
                    selection_span: param.span.clone(),
                    children: Vec::new(),
                })
                .collect(),
        }),
        Item::Type(decl) => Some(RssDocumentSymbol {
            name: decl.name.clone(),
            kind: SymbolKind::Type,
            detail: Some(type_kind_detail(decl.kind).to_string()),
            span: decl.span.clone(),
            selection_span: decl.span.clone(),
            children: decl
                .fields
                .iter()
                .map(|field| RssDocumentSymbol {
                    name: field.name.clone(),
                    kind: SymbolKind::Field,
                    detail: Some(type_ref_display(&field.ty)),
                    span: field.span.clone(),
                    selection_span: field.span.clone(),
                    children: Vec::new(),
                })
                .collect(),
        }),
        Item::SumType(decl) => Some(RssDocumentSymbol {
            name: decl.name.clone(),
            kind: SymbolKind::Type,
            detail: Some("sum".to_string()),
            span: decl.span.clone(),
            selection_span: decl.span.clone(),
            children: decl
                .variants
                .iter()
                .map(|variant| RssDocumentSymbol {
                    name: variant.name.clone(),
                    kind: SymbolKind::Variant,
                    detail: None,
                    span: variant.span.clone(),
                    selection_span: variant.span.clone(),
                    children: variant
                        .fields
                        .iter()
                        .map(|field| RssDocumentSymbol {
                            name: field.name.clone(),
                            kind: SymbolKind::Field,
                            detail: Some(type_ref_display(&field.ty)),
                            span: field.span.clone(),
                            selection_span: field.span.clone(),
                            children: Vec::new(),
                        })
                        .collect(),
                })
                .collect(),
        }),
        Item::TypeAlias(decl) => Some(RssDocumentSymbol {
            name: decl.name.clone(),
            kind: SymbolKind::Type,
            detail: Some(format!("alias = {}", type_ref_display(&decl.target))),
            span: decl.span.clone(),
            selection_span: decl.span.clone(),
            children: Vec::new(),
        }),
        Item::Const(decl) => Some(RssDocumentSymbol {
            name: decl.name.clone(),
            kind: SymbolKind::Const,
            detail: decl.type_annotation.as_ref().map(type_ref_display),
            span: decl.span.clone(),
            selection_span: decl.span.clone(),
            children: Vec::new(),
        }),
        Item::Module(_) | Item::Use(_) => None,
    }
}

fn function_detail(function: &FunctionDecl) -> String {
    let params = function
        .params
        .iter()
        .map(param_detail)
        .collect::<Vec<_>>()
        .join(", ");
    let return_ty = function
        .return_ty
        .as_ref()
        .map(type_ref_display)
        .unwrap_or_else(|| "Unit".to_string());
    let async_prefix = if function.is_async { "async " } else { "" };
    let native_prefix = if function.is_native { "native " } else { "" };
    let effects = if function.effects.is_empty() {
        String::new()
    } else {
        format!(
            " effects({})",
            function
                .effects
                .iter()
                .map(effect_display)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!("{async_prefix}{native_prefix}fn({params}) -> {return_ty}{effects}")
}

fn param_detail(param: &Param) -> String {
    let effect = param
        .effect
        .map(|effect| format!("{} ", effect.as_str()))
        .unwrap_or_default();
    format!("{}: {effect}{}", param.name, type_ref_display(&param.ty))
}

fn field_detail(field: &FieldDecl) -> String {
    format!("{}: {}", field.name, type_ref_display(&field.ty))
}

fn type_kind_detail(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Resource => "resource",
    }
}

fn effect_display(effect: &crate::syntax::ast::EffectDecl) -> String {
    match effect {
        crate::syntax::ast::EffectDecl::Name(name) => name.clone(),
        crate::syntax::ast::EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn type_ref_display(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_display)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_display(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_display)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let owned = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {owned}")
    } else {
        owned
    }
}

fn span_contains(span: &Span, line: usize, column: usize) -> bool {
    span.line == line && column >= span.column && column < span.column + span.length.max(1)
}

fn name_span_after(source: &str, span: &Span, name: &str) -> Span {
    let Some(line) = source.lines().nth(span.line.saturating_sub(1)) else {
        return span.clone();
    };
    let start_char = span.column.saturating_sub(1);
    let start_byte = line
        .char_indices()
        .nth(start_char)
        .map(|(index, _)| index)
        .unwrap_or(line.len());
    let Some(relative_byte) = line[start_byte..].find(name) else {
        return span.clone();
    };
    let before_name = &line[..start_byte + relative_byte];
    Span {
        file: span.file.clone(),
        line: span.line,
        column: before_name.chars().count() + 1,
        length: name.chars().count(),
    }
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

struct Builder<'a> {
    source: &'a str,
    definitions: Vec<Definition>,
    references: Vec<Reference>,
    scopes: Vec<HashMap<String, usize>>,
}

impl Builder<'_> {
    fn define(
        &mut self,
        name: &str,
        kind: SymbolKind,
        span: &Span,
        detail: Option<String>,
    ) -> Option<usize> {
        if name.is_empty() {
            return None;
        }
        let span = name_span_after(self.source, span, name);
        let index = self.definitions.len();
        self.definitions.push(Definition {
            name: name.to_string(),
            kind,
            span,
            detail,
        });
        self.scopes
            .last_mut()
            .expect("symbol builder always has a scope")
            .insert(name.to_string(), index);
        Some(index)
    }

    fn reference(&mut self, name: &str, span: Span, is_type: bool) {
        if name.is_empty() {
            return;
        }
        let definition = self.resolve(name, is_type);
        self.references.push(Reference {
            name: name.to_string(),
            span,
            is_type,
            definition,
        });
    }

    fn visit_program(&mut self, program: &Program) {
        for item in &program.items {
            self.define_top_level_item(item);
        }
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn define_top_level_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => {
                self.define(
                    &function.name,
                    SymbolKind::Function,
                    &function.span,
                    Some(function_detail(function)),
                );
            }
            Item::Type(decl) => {
                self.define(
                    &decl.name,
                    SymbolKind::Type,
                    &decl.span,
                    Some(type_kind_detail(decl.kind).to_string()),
                );
            }
            Item::SumType(decl) => {
                self.define(
                    &decl.name,
                    SymbolKind::Type,
                    &decl.span,
                    Some("sum".to_string()),
                );
                for variant in &decl.variants {
                    let detail = if variant.fields.is_empty() {
                        None
                    } else {
                        Some(format!(
                            "({})",
                            variant
                                .fields
                                .iter()
                                .map(field_detail)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    };
                    self.define(&variant.name, SymbolKind::Variant, &variant.span, detail);
                }
            }
            Item::TypeAlias(decl) => {
                self.define(
                    &decl.name,
                    SymbolKind::Type,
                    &decl.span,
                    Some(format!("alias = {}", type_ref_display(&decl.target))),
                );
            }
            Item::Const(decl) => {
                self.define(
                    &decl.name,
                    SymbolKind::Const,
                    &decl.span,
                    decl.type_annotation.as_ref().map(type_ref_display),
                );
            }
            Item::Module(_) | Item::Use(_) => {}
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
        self.push_scope();
        for param in &function.params {
            self.define(
                &param.name,
                SymbolKind::Param,
                &param.span,
                Some(param_detail(param)),
            );
            self.visit_type_ref(&param.ty);
        }
        if let Some(return_ty) = &function.return_ty {
            self.visit_type_ref(return_ty);
        }
        self.visit_block(&function.body);
        self.pop_scope();
    }

    fn visit_type_decl(&mut self, decl: &TypeDecl) {
        for field in &decl.fields {
            self.visit_type_ref(&field.ty);
        }
        if let Some(body) = &decl.drop_body {
            self.visit_block(body);
        }
    }

    fn visit_sum_type(&mut self, decl: &SumTypeDecl) {
        for variant in &decl.variants {
            for field in &variant.fields {
                self.visit_type_ref(&field.ty);
            }
        }
    }

    fn visit_type_alias(&mut self, decl: &TypeAliasDecl) {
        self.visit_type_ref(&decl.target);
    }

    fn visit_const(&mut self, decl: &ConstDecl) {
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
        self.push_scope();
        for statement in &block.statements {
            self.visit_stmt(statement);
        }
        self.pop_scope();
    }

    fn visit_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(let_stmt) => {
                if let Some(type_annotation) = &let_stmt.type_annotation {
                    self.visit_type_ref(type_annotation);
                }
                if let Some(value) = &let_stmt.value {
                    self.visit_expr(value);
                }
                self.define(
                    &let_stmt.name,
                    SymbolKind::Local,
                    &let_stmt.span,
                    let_stmt.type_annotation.as_ref().map(type_ref_display),
                );
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
                self.push_scope();
                self.define(&for_stmt.binding, SymbolKind::Local, &for_stmt.span, None);
                self.visit_block(&for_stmt.body);
                self.pop_scope();
            }
            Stmt::Match(match_stmt) => {
                self.visit_expr(&match_stmt.value);
                for arm in &match_stmt.arms {
                    self.push_scope();
                    self.visit_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                    self.visit_block(&arm.body);
                    self.pop_scope();
                }
            }
            Stmt::TaskGroup(task_group) => self.visit_block(&task_group.body),
            Stmt::Select(select) => {
                for arm in &select.arms {
                    self.visit_expr(&arm.operation);
                    self.push_scope();
                    if arm.binding != "_" {
                        self.define(&arm.binding, SymbolKind::Local, &arm.span, None);
                    }
                    self.visit_block(&arm.body);
                    self.pop_scope();
                }
            }
            Stmt::LetElse(let_else) => {
                self.visit_expr(&let_else.value);
                self.define(
                    &let_else.binding_name,
                    SymbolKind::Local,
                    &let_else.span,
                    None,
                );
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
        match pattern {
            MatchPattern::Binding { name, span } => {
                self.define(name, SymbolKind::Local, span, None);
            }
            MatchPattern::Variant {
                binding: Some(binding),
                ..
            } => self.visit_pattern(binding),
            MatchPattern::Struct { fields, .. } => {
                for field in fields {
                    if let Some(pattern) = &field.pattern {
                        self.visit_pattern(pattern);
                    } else if !field.ignored
                        && let Some(binding) = &field.binding
                    {
                        self.define(binding, SymbolKind::Local, &field.span, None);
                    }
                }
            }
            MatchPattern::List {
                prefix,
                rest,
                suffix,
                span,
            } => {
                for pattern in prefix.iter().chain(suffix) {
                    self.visit_pattern(pattern);
                }
                if let Some(Some(rest_name)) = rest {
                    self.define(rest_name, SymbolKind::Local, span, None);
                }
            }
            MatchPattern::Variant { binding: None, .. }
            | MatchPattern::Literal { .. }
            | MatchPattern::Wildcard(_) => {}
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
                    self.push_scope();
                    self.visit_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                    self.visit_block(&arm.body);
                    self.pop_scope();
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

    fn resolve(&self, name: &str, prefer_type: bool) -> Option<usize> {
        if prefer_type {
            for scope in self.scopes.iter().rev() {
                if let Some(index) = scope.get(name)
                    && self
                        .definitions
                        .get(*index)
                        .is_some_and(|definition| definition.kind == SymbolKind::Type)
                {
                    return Some(*index);
                }
            }
        }
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_inventory_carries_module_qualname_and_lowered_name() {
        let source = concat!(
            "const MAX_RETRIES: Int = 3\n",
            "struct Device { id: Int }\n",
            "fn Device.open(id: Int) -> fresh Device {\n",
            "    return Device(id: id)\n",
            "}\n",
        );
        let inventory = symbol_inventory("helpers.rss", source);
        let find = |qualname: &str| {
            inventory
                .iter()
                .find(|entry| entry.qualname == qualname)
                .unwrap_or_else(|| panic!("missing `{qualname}` in inventory: {inventory:?}"))
        };

        // Every entry is tagged with the source module.
        assert!(inventory.iter().all(|entry| entry.module == "helpers"));

        // A dotted member function lowers to a flattened Rust symbol.
        let open = find("Device.open");
        assert_eq!(open.kind, SymbolKind::Function);
        assert_eq!(open.lowered_name, "Device_open");

        // Const and type symbols keep their (keyword-safe) names.
        assert_eq!(find("MAX_RETRIES").kind, SymbolKind::Const);
        assert_eq!(find("MAX_RETRIES").lowered_name, "MAX_RETRIES");
        assert_eq!(find("Device").kind, SymbolKind::Type);
    }

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

    #[test]
    fn returns_symbol_info_at_reference() {
        let index = symbol_index("t.rss", SOURCE);
        let symbol = index.symbol_at(6, 18).expect("resolves helper symbol");

        assert_eq!(symbol.name, "helper");
        assert_eq!(symbol.kind, SymbolKind::Function);
        assert_eq!(symbol.span.line, 1);
        assert_eq!(symbol.detail.as_deref(), Some("fn(value: read Int) -> Int"));
    }

    #[test]
    fn symbol_info_includes_type_and_local_details() {
        let source = concat!(
            "struct User {\n",
            "    name: String\n",
            "}\n",
            "\n",
            "fn run(user: read User) -> Unit {\n",
            "    let label: String = user.name\n",
            "    return\n",
            "}\n",
        );
        let index = symbol_index("detail.rss", source);
        let user = index.symbol_at(5, 19).expect("User type symbol");
        let label = index
            .definitions()
            .iter()
            .find(|definition| definition.name == "label")
            .expect("label local definition");

        assert_eq!(user.kind, SymbolKind::Type);
        assert_eq!(user.detail.as_deref(), Some("struct"));
        assert_eq!(label.kind, SymbolKind::Local);
        assert_eq!(label.detail.as_deref(), Some("String"));
    }

    #[test]
    fn builds_document_symbols_for_outline() {
        let source = concat!(
            "struct User {\n",
            "    name: String\n",
            "}\n",
            "\n",
            "sum Status {\n",
            "    Ready\n",
            "    Failed(reason: String)\n",
            "}\n",
            "\n",
            "fn greet(user: read User) -> String {\n",
            "    return user.name\n",
            "}\n",
        );

        let symbols = document_symbols("outline.rss", source);

        let user = symbols
            .iter()
            .find(|symbol| symbol.name == "User")
            .expect("User outline symbol");
        assert_eq!(user.kind, SymbolKind::Type);
        assert_eq!(user.detail.as_deref(), Some("struct"));
        assert!(user.children.iter().any(|child| {
            child.name == "name"
                && child.kind == SymbolKind::Field
                && child.detail.as_deref() == Some("String")
        }));

        let status = symbols
            .iter()
            .find(|symbol| symbol.name == "Status")
            .expect("Status outline symbol");
        assert!(
            status
                .children
                .iter()
                .any(|child| { child.name == "Failed" && child.kind == SymbolKind::Variant })
        );

        let greet = symbols
            .iter()
            .find(|symbol| symbol.name == "greet")
            .expect("greet outline symbol");
        assert_eq!(greet.kind, SymbolKind::Function);
        assert_eq!(greet.children[0].name, "user");
        assert!(
            greet
                .detail
                .as_ref()
                .is_some_and(|detail| detail.contains("fn(user: read User) -> String")),
            "{greet:?}"
        );
    }

    #[test]
    fn references_do_not_cross_function_scopes_for_same_parameter_name() {
        let source = concat!(
            "fn first(value: Int) -> Int {\n",
            "    return value\n",
            "}\n",
            "\n",
            "fn second(value: Int) -> Int {\n",
            "    return value\n",
            "}\n",
        );
        let index = symbol_index("scopes.rss", source);
        let first_refs = index.references_at(2, 12, true);
        let second_refs = index.references_at(6, 12, true);

        assert!(first_refs.iter().any(|span| span.line == 1));
        assert!(first_refs.iter().any(|span| span.line == 2));
        assert!(!first_refs.iter().any(|span| span.line == 5));
        assert!(!first_refs.iter().any(|span| span.line == 6));

        assert!(second_refs.iter().any(|span| span.line == 5));
        assert!(second_refs.iter().any(|span| span.line == 6));
        assert!(!second_refs.iter().any(|span| span.line == 1));
        assert!(!second_refs.iter().any(|span| span.line == 2));
    }

    #[test]
    fn definition_prefers_nearest_shadowing_binding() {
        let source = concat!(
            "fn run(value: Int) -> Int {\n",
            "    if true {\n",
            "        let value = 2\n",
            "        return value\n",
            "    }\n",
            "    return value\n",
            "}\n",
        );
        let index = symbol_index("shadow.rss", source);
        let inner = index
            .definition_at(4, 16)
            .expect("inner value should resolve");
        let outer = index
            .definition_at(6, 12)
            .expect("outer value should resolve");

        assert_eq!(inner.line, 3);
        assert_eq!(outer.line, 1);
    }

    #[test]
    fn lookup_at_exposes_unresolved_cross_file_symbol_key() {
        let source = concat!(
            "fn run() -> Int {\n",
            "    return helper(value: 1)\n",
            "}\n",
        );
        let index = symbol_index("caller.rss", source);
        let lookup = index.lookup_at(2, 12).expect("helper lookup");

        assert_eq!(lookup.name, "helper");
        assert!(!lookup.is_type);
        assert!(lookup.local_definition.is_none());
    }
}
