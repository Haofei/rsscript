use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::analyzer::analyze_source_with_core;
use crate::diagnostic::{Diagnostic, Severity, Span, code};
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature,
    FunctionDecl, GenericBound, GenericParam, Item, LetKind, Param, Program, Stmt, TypeDecl,
    TypeKind, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRustPackage {
    pub package_name: String,
    pub cargo_toml: String,
    pub lib_rs: String,
    pub main_rs: Option<String>,
    pub source_map_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredRust {
    pub rust_source: String,
    pub source_map: Vec<RustSourceMapEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RustSourceMapEntry {
    pub kind: String,
    pub source: Span,
    pub generated: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemappedRustcDiagnostic {
    pub diagnostic: Diagnostic,
    pub mapped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustBackendCheckResult {
    pub success: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub cargo_status: Option<i32>,
    pub stderr: String,
}

pub fn lower_source_to_rust(file: &str, source: &str) -> Result<String, Vec<Diagnostic>> {
    lower_source_to_rust_with_map(file, source).map(|lowered| lowered.rust_source)
}

pub fn lower_source_to_rust_with_map(
    file: &str,
    source: &str,
) -> Result<LoweredRust, Vec<Diagnostic>> {
    let diagnostics = analyze_source_with_core(file, source);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return Err(diagnostics);
    }

    let program = parse_source(file, source);
    Ok(lower_program_to_rust_with_map(&program))
}

pub fn lower_source_to_rust_package(
    file: &str,
    source: &str,
    package_name: &str,
    runtime_path: &str,
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    let diagnostics = analyze_source_with_core(file, source);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return Err(diagnostics);
    }

    let program = parse_source(file, source);
    let lowered = lower_program_to_rust_with_map(&program);
    let package_name = cargo_package_name(package_name);
    let cargo_toml = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nrsscript-runtime = {{ path = \"{}\" }}\n",
        toml_string(runtime_path)
    );
    let main_rs = rust_package_main(&program, &package_name);
    let source_map_json =
        serde_json::to_string_pretty(&lowered.source_map).expect("source map should serialize");

    Ok(GeneratedRustPackage {
        package_name,
        cargo_toml,
        lib_rs: lowered.rust_source,
        main_rs,
        source_map_json,
    })
}

pub fn write_generated_rust_package(
    out_dir: &Path,
    package: &GeneratedRustPackage,
) -> Result<(), String> {
    let src_dir = out_dir.join("src");
    fs::create_dir_all(&src_dir)
        .map_err(|error| format!("failed to create {}: {error}", src_dir.display()))?;
    fs::write(out_dir.join("Cargo.toml"), &package.cargo_toml).map_err(|error| {
        format!(
            "failed to write {}: {error}",
            out_dir.join("Cargo.toml").display()
        )
    })?;
    fs::write(src_dir.join("lib.rs"), &package.lib_rs).map_err(|error| {
        format!(
            "failed to write {}: {error}",
            src_dir.join("lib.rs").display()
        )
    })?;
    if let Some(main_rs) = &package.main_rs {
        fs::write(src_dir.join("main.rs"), main_rs).map_err(|error| {
            format!(
                "failed to write {}: {error}",
                src_dir.join("main.rs").display()
            )
        })?;
    }
    fs::write(
        out_dir.join("rsscript-source-map.json"),
        &package.source_map_json,
    )
    .map_err(|error| {
        format!(
            "failed to write {}: {error}",
            out_dir.join("rsscript-source-map.json").display()
        )
    })?;
    Ok(())
}

pub fn lower_program_to_rust(program: &Program) -> String {
    lower_program_to_rust_with_map(program).rust_source
}

pub fn lower_program_to_rust_with_map(program: &Program) -> LoweredRust {
    RustLowerer::new(program).lower()
}

pub fn parse_source_map_json(source_map_json: &str) -> Result<Vec<RustSourceMapEntry>, String> {
    serde_json::from_str(source_map_json)
        .map_err(|error| format!("failed to parse RSScript source map JSON: {error}"))
}

pub fn remap_rustc_diagnostic_json(
    source_map: &[RustSourceMapEntry],
    rustc_json: &str,
) -> Result<Option<RemappedRustcDiagnostic>, String> {
    let value: serde_json::Value = serde_json::from_str(rustc_json)
        .map_err(|error| format!("failed to parse rustc JSON line: {error}"))?;
    let Some(value) = rustc_diagnostic_value(&value) else {
        return Ok(None);
    };
    let rustc: RustcJsonDiagnostic = serde_json::from_value(value.clone())
        .map_err(|error| format!("failed to parse rustc JSON diagnostic: {error}"))?;
    if !matches!(rustc.level.as_str(), "error" | "warning") {
        return Ok(None);
    }

    let rustc_span = rustc
        .spans
        .iter()
        .find(|span| span.is_primary)
        .or_else(|| rustc.spans.first());
    let backend_code = rustc
        .code
        .as_ref()
        .map(|code| code.code.as_str())
        .unwrap_or("<none>");

    if let Some(rustc_span) = rustc_span
        && let Some(entry) = best_source_map_entry(
            source_map,
            &rustc_span.file_name,
            rustc_span.line_start,
            rustc_span.column_start,
        )
    {
        let severity = rustc_severity(&rustc.level);
        let summary = format!("backend diagnostic mapped to RSScript: {}", rustc.message);
        let diagnostic = Diagnostic {
            code: code::RUSTC_DIAGNOSTIC_MAPPED.to_string(),
            severity,
            summary,
            span: entry.source.clone(),
            label: "backend diagnostic maps to this RSScript construct".to_string(),
            causes: vec![
                format!("rustc code: {backend_code}"),
                format!(
                    "generated Rust: {}:{}:{}",
                    rustc_span.file_name, rustc_span.line_start, rustc_span.column_start
                ),
                rustc.message,
            ],
            fixes: Vec::new(),
        };
        return Ok(Some(RemappedRustcDiagnostic {
            diagnostic,
            mapped: true,
        }));
    }

    let generated = rustc_span
        .map(generated_span_from_rustc)
        .unwrap_or_else(|| Span {
            file: "<rustc-json>".to_string(),
            line: 1,
            column: 1,
            length: 1,
        });
    let diagnostic = Diagnostic {
        code: code::RUSTC_DIAGNOSTIC_UNMAPPABLE.to_string(),
        severity: rustc_severity(&rustc.level),
        summary: format!("unmappable backend diagnostic: {}", rustc.message),
        span: generated,
        label: "generated Rust diagnostic could not be mapped to RSScript source".to_string(),
        causes: vec![format!("rustc code: {backend_code}"), rustc.message],
        fixes: Vec::new(),
    };
    Ok(Some(RemappedRustcDiagnostic {
        diagnostic,
        mapped: false,
    }))
}

pub fn remap_rustc_diagnostic_json_lines(
    source_map: &[RustSourceMapEntry],
    rustc_json_lines: &str,
) -> Result<Vec<RemappedRustcDiagnostic>, String> {
    let mut diagnostics = Vec::new();
    for line in rustc_json_lines
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        if let Some(diagnostic) = remap_rustc_diagnostic_json(source_map, line)? {
            diagnostics.push(diagnostic);
        }
    }
    Ok(diagnostics)
}

pub fn check_generated_rust_package(package_dir: &Path) -> Result<RustBackendCheckResult, String> {
    let source_map_json = fs::read_to_string(package_dir.join("rsscript-source-map.json"))
        .map_err(|error| {
            format!(
                "failed to read {}: {error}",
                package_dir.join("rsscript-source-map.json").display()
            )
        })?;
    let source_map = parse_source_map_json(&source_map_json)?;
    let manifest_path = package_dir.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("check")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--message-format=json")
        .output()
        .map_err(|error| format!("failed to run cargo check: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let remapped = remap_rustc_diagnostic_json_lines(&source_map, &stdout)?;
    let diagnostics = remapped
        .into_iter()
        .map(|remapped| remapped.diagnostic)
        .collect();

    Ok(RustBackendCheckResult {
        success: output.status.success(),
        diagnostics,
        cargo_status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

struct RustLowerer<'a> {
    program: &'a Program,
    type_kinds: BTreeMap<String, TypeKind>,
    param_effects: BTreeMap<String, DataEffect>,
    mutated_bindings: BTreeSet<String>,
    source_map: Vec<RustSourceMapEntry>,
}

impl<'a> RustLowerer<'a> {
    fn new(program: &'a Program) -> Self {
        let type_kinds = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Type(ty) => Some((ty.name.clone(), ty.kind)),
                Item::Function(_) => None,
            })
            .collect();

        Self {
            program,
            type_kinds,
            param_effects: BTreeMap::new(),
            mutated_bindings: BTreeSet::new(),
            source_map: Vec::new(),
        }
    }

    fn lower(mut self) -> LoweredRust {
        let mut out = String::new();
        out.push_str("// Generated by RSScript. Edit the .rss source instead.\n");
        out.push_str(
            "// Runtime hooks are intentionally explicit while Rust lowering is stabilizing.\n",
        );
        if self.program.has_feature(FileFeature::Local) {
            out.push_str("// RSScript features: local\n");
        } else {
            out.push_str("// RSScript features: <none>\n");
        }
        out.push('\n');

        for (index, item) in self.program.items.iter().enumerate() {
            match item {
                Item::Type(ty) => self.lower_type_decl(ty, &mut out),
                Item::Function(function) => self.lower_function(function, &mut out),
            }
            if index + 1 < self.program.items.len() {
                out.push('\n');
            }
        }

        LoweredRust {
            rust_source: out,
            source_map: self.source_map,
        }
    }

    fn lower_type_decl(&mut self, ty: &TypeDecl, out: &mut String) {
        self.record_source_marker(out, 0, "type", &ty.span);
        if ty.kind == TypeKind::Resource {
            out.push_str("#[must_use]\n");
        }
        out.push_str("#[derive(Debug)]\n");
        out.push_str(&format!(
            "{}struct {}{} {{\n",
            visibility(true),
            rust_ident(&ty.name),
            lower_generic_params(&ty.type_params)
        ));
        for field in &ty.fields {
            self.lower_field_decl(field, out);
        }
        out.push_str("}\n");

        if ty.kind == TypeKind::Resource {
            out.push_str(&format!(
                "\nimpl{} rsscript_runtime::Resource for {}{} {{}}\n",
                lower_impl_generics(&ty.type_params),
                rust_ident(&ty.name),
                lower_generic_args(&ty.type_params)
            ));
            out.push_str(&format!(
                "\nimpl{} Drop for {}{} {{\n",
                lower_impl_generics(&ty.type_params),
                rust_ident(&ty.name),
                lower_generic_args(&ty.type_params)
            ));
            out.push_str("    fn drop(&mut self) {\n");
            out.push_str(
                "        // RSScript resource drop body is lowered by the runtime-aware pass.\n",
            );
            out.push_str("    }\n");
            out.push_str("}\n");
        }
    }

    fn lower_field_decl(&mut self, field: &FieldDecl, out: &mut String) {
        let rust_ty = self.lower_type_ref(&field.ty, ManagedPosition::Field);
        self.source_map.push(RustSourceMapEntry {
            kind: "field".to_string(),
            source: field.span.clone(),
            generated: generated_span_at_end(out, "src/lib.rs", "field"),
        });
        if field.is_handle {
            out.push_str(&format!(
                "    pub {}: rsscript_runtime::Gc<{}>,\n",
                rust_ident(&field.name),
                rust_ty
            ));
        } else {
            out.push_str(&format!(
                "    pub {}: {},\n",
                rust_ident(&field.name),
                rust_ty
            ));
        }
    }

    fn lower_function(&mut self, function: &FunctionDecl, out: &mut String) {
        let previous_param_effects = std::mem::take(&mut self.param_effects);
        let previous_mutated_bindings = std::mem::take(&mut self.mutated_bindings);
        self.param_effects = function
            .params
            .iter()
            .filter_map(|param| param.effect.map(|effect| (param.name.clone(), effect)))
            .collect();
        self.mutated_bindings = collect_mutated_bindings(&function.body);
        self.record_source_marker(out, 0, "function", &function.span);
        let async_prefix = if function.is_async { "async " } else { "" };
        let is_public = function.is_public || is_runnable_main(function);
        out.push_str(&format!(
            "{}{}fn {}{}(",
            visibility(is_public),
            async_prefix,
            rust_ident(&function.name),
            lower_generic_params(&function.type_params)
        ));
        out.push_str(
            &function
                .params
                .iter()
                .map(|param| self.lower_param(param))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push(')');
        if let Some(return_ty) = &function.return_ty {
            out.push_str(" -> ");
            out.push_str(&self.lower_return_type(return_ty, function.returns_fresh));
        }
        out.push_str(" {\n");
        if function.effects.iter().any(is_native_boundary) {
            out.push_str(
                "    // RSScript native/unsafe boundary: review before binding implementation.\n",
            );
        }
        if function.body.statements.is_empty() {
            for param in &function.params {
                out.push_str(&format!("    let _ = &{};\n", rust_ident(&param.name)));
            }
            out.push_str("    todo!(\"RSScript interface function\");\n");
        } else {
            self.lower_block(&function.body, out, 1);
        }
        out.push_str("}\n");
        self.param_effects = previous_param_effects;
        self.mutated_bindings = previous_mutated_bindings;
    }

    fn lower_param(&self, param: &Param) -> String {
        let ty = self.lower_type_ref(&param.ty, ManagedPosition::Param);
        let rust_ty = match param.effect {
            Some(DataEffect::Read) => format!("&{ty}"),
            Some(DataEffect::Mut) => format!("&mut {ty}"),
            Some(DataEffect::Take) | None => ty,
        };
        format!("{}: {}", rust_ident(&param.name), rust_ty)
    }

    fn lower_return_type(&self, ty: &TypeRef, returns_fresh: bool) -> String {
        let position = if returns_fresh {
            ManagedPosition::FreshReturn
        } else {
            ManagedPosition::Return
        };
        self.lower_type_ref(ty, position)
    }

    fn lower_block(&mut self, block: &Block, out: &mut String, indent: usize) {
        for statement in &block.statements {
            self.lower_stmt(statement, out, indent);
        }
    }

    fn lower_stmt(&mut self, statement: &Stmt, out: &mut String, indent: usize) {
        let pad = "    ".repeat(indent);
        let marker = self.record_source_marker(out, indent, "statement", stmt_span(statement));
        self.record_statement_source_map(statement, &marker.generated);
        match statement {
            Stmt::Let(stmt) => {
                let mutable =
                    if stmt.kind == LetKind::Local || self.mutated_bindings.contains(&stmt.name) {
                        "mut "
                    } else {
                        ""
                    };
                if let Some(value) = &stmt.value {
                    out.push_str(&format!(
                        "{pad}let {mutable}{} = {};\n",
                        rust_ident(&stmt.name),
                        self.lower_expr(value)
                    ));
                } else {
                    out.push_str(&format!("{pad}let {mutable}{};\n", rust_ident(&stmt.name)));
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    out.push_str(&format!("{pad}return {};\n", self.lower_expr(value)));
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
                out.push_str(&format!(
                    "{pad}let mut {} = {};\n",
                    rust_ident(&stmt.binding),
                    resource
                ));
                out.push_str(&format!("{pad}{{\n"));
                self.lower_block(&stmt.body, out, indent + 1);
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
            Stmt::Break(_) => out.push_str(&format!("{pad}break;\n")),
            Stmt::Continue(_) => out.push_str(&format!("{pad}continue;\n")),
            Stmt::Expr(expr) => out.push_str(&format!("{pad}{};\n", self.lower_expr(expr))),
            Stmt::Unknown(_) => out.push_str(&format!(
                "{pad}todo!(\"unsupported RSScript statement\");\n"
            )),
        }
    }

    fn record_source_marker(
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

    fn record_statement_source_map(&mut self, statement: &Stmt, generated: &Span) {
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
            Stmt::Expr(expr) => self.record_expr_source_map(expr, generated),
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
        }
    }

    fn record_block_source_map(&mut self, block: &Block, generated: &Span) {
        for statement in &block.statements {
            self.source_map.push(RustSourceMapEntry {
                kind: "statement".to_string(),
                source: stmt_span(statement).clone(),
                generated: generated.clone(),
            });
            self.record_statement_source_map(statement, generated);
        }
    }

    fn record_expr_source_map(&mut self, expr: &Expr, generated: &Span) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.record_expr_source_map(left, generated);
                self.record_expr_source_map(right, generated);
            }
            Expr::Field { base, span, .. } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "field_path".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                });
                self.record_expr_source_map(base, generated);
            }
            Expr::Index { base, index, .. } => {
                self.record_expr_source_map(base, generated);
                self.record_expr_source_map(index, generated);
            }
            Expr::Call { args, span, .. } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "call".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                });
                for arg in args {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "named_arg".to_string(),
                        source: arg.span.clone(),
                        generated: generated.clone(),
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
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Try { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "try".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Closure { body, .. } => self.record_block_source_map(body, generated),
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn lower_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(name, _) => lower_builtin_value_ident(name)
                .map(str::to_string)
                .unwrap_or_else(|| rust_ident(name)),
            Expr::Number(value, _) => value.clone(),
            Expr::String(value, _) => format!("{:?}.to_string()", decode_string_token(value)),
            Expr::Binary {
                op, left, right, ..
            } => {
                let op = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Subtract => "-",
                    BinaryOp::Multiply => "*",
                    BinaryOp::Divide => "/",
                    BinaryOp::Equal => "==",
                    BinaryOp::NotEqual => "!=",
                    BinaryOp::Less => "<",
                    BinaryOp::LessEqual => "<=",
                    BinaryOp::Greater => ">",
                    BinaryOp::GreaterEqual => ">=",
                    BinaryOp::LogicalAnd => "&&",
                    BinaryOp::LogicalOr => "||",
                };
                format!("{} {op} {}", self.lower_expr(left), self.lower_expr(right))
            }
            Expr::Field { base, name, .. } => {
                format!("{}.{}", self.lower_expr(base), rust_ident(name))
            }
            Expr::Index { base, index, .. } => {
                format!("{}[{}]", self.lower_expr(base), self.lower_expr(index))
            }
            Expr::Call { callee, args, span } => {
                if let Callee::Name(name) = callee {
                    if self.type_kinds.contains_key(name) {
                        let fields = args
                            .iter()
                            .map(|arg| {
                                let field = arg
                                    .name
                                    .as_deref()
                                    .map(rust_ident)
                                    .unwrap_or_else(|| "/* unnamed */".to_string());
                                format!("{field}: {}", self.lower_expr(&arg.value))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        return format!("{} {{ {fields} }}", rust_ident(name));
                    }

                    if is_rust_enum_constructor(name) {
                        let args = args
                            .iter()
                            .map(|arg| self.lower_expr(&arg.value))
                            .collect::<Vec<_>>()
                            .join(", ");
                        return format!("{}({args})", rust_ident(name));
                    }
                }
                if is_string_concat_callee(callee) {
                    return lower_string_concat_call(self, args);
                }
                if is_resource_pool_new_callee(callee) {
                    return lower_resource_pool_new_call(self, args);
                }
                let is_resource_pool_borrow = is_resource_pool_borrow_callee(callee);
                let callee = if is_resource_pool_borrow {
                    "rsscript_runtime::ResourcePool::borrow_at".to_string()
                } else {
                    lower_callee(callee)
                };
                let mut args = args
                    .iter()
                    .map(|arg| self.lower_expr(&arg.value))
                    .collect::<Vec<_>>();
                if is_resource_pool_borrow {
                    args.push(lower_source_span(span));
                }
                let args = args.join(", ");
                format!("{callee}({args})")
            }
            Expr::Effect { effect, value, .. } => match effect {
                DataEffect::Read => format!("&{}", self.lower_expr(value)),
                DataEffect::Mut => {
                    if let Expr::Ident(name, _) = &**value
                        && self.param_effects.get(name) == Some(&DataEffect::Mut)
                    {
                        rust_ident(name)
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
            Expr::Try { value, .. } => format!("{}?", self.lower_expr(value)),
            Expr::Closure { body, .. } => {
                if let [Stmt::Expr(value)] = body.statements.as_slice() {
                    return format!("|| {}", self.lower_expr(value));
                }
                let mut out = String::new();
                out.push_str("|| {\n");
                self.lower_block(body, &mut out, 1);
                out.push('}');
                out
            }
            Expr::Unknown(_) => "todo!(\"unsupported RSScript expression\")".to_string(),
        }
    }

    fn lower_type_ref(&self, ty: &TypeRef, position: ManagedPosition) -> String {
        let lowered = match ty.name.as_str() {
            "Unit" => "()".to_string(),
            "Bool" => "bool".to_string(),
            "Byte" => "u8".to_string(),
            "Char" => "char".to_string(),
            "Int" => "i64".to_string(),
            "Int8" => "i8".to_string(),
            "Int16" => "i16".to_string(),
            "Int32" => "i32".to_string(),
            "Int64" => "i64".to_string(),
            "UInt" => "u64".to_string(),
            "UInt8" => "u8".to_string(),
            "UInt16" => "u16".to_string(),
            "UInt32" => "u32".to_string(),
            "UInt64" => "u64".to_string(),
            "Float" => "f64".to_string(),
            "Float32" => "f32".to_string(),
            "Float64" => "f64".to_string(),
            "String" => "String".to_string(),
            "Url" => "String".to_string(),
            "Fd" => "i64".to_string(),
            "Bytes" | "Buffer" => "Vec<u8>".to_string(),
            "Path" => "std::path::PathBuf".to_string(),
            "Counter" => "rsscript_runtime::Counter".to_string(),
            "File" => "rsscript_runtime::File".to_string(),
            "FileError" | "IOError" => "std::io::Error".to_string(),
            "Request" => "rsscript_runtime::Request".to_string(),
            "Response" => "rsscript_runtime::Response".to_string(),
            "HttpError" => "rsscript_runtime::HttpError".to_string(),
            "ConfigValue" => "rsscript_runtime::ConfigValue".to_string(),
            "ConfigStore" => "rsscript_runtime::ConfigStore".to_string(),
            "ConfigError" => "rsscript_runtime::ConfigError".to_string(),
            "DbConnection" => "rsscript_runtime::DbConnection".to_string(),
            "DbError" => "rsscript_runtime::DbError".to_string(),
            "Image" => "rsscript_runtime::Image".to_string(),
            "ImageError" => "rsscript_runtime::ImageError".to_string(),
            "JsonValue" => "rsscript_runtime::JsonValue".to_string(),
            "JsonError" => "rsscript_runtime::JsonError".to_string(),
            "RowBuffer" => "rsscript_runtime::RowBuffer".to_string(),
            "Row" => "rsscript_runtime::Row".to_string(),
            "CsvError" => "rsscript_runtime::CsvError".to_string(),
            "Result" if ty.args.len() == 2 => format!(
                "Result<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "Option" if ty.args.len() == 1 => {
                format!(
                    "Option<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "List" if ty.args.len() == 1 => {
                format!(
                    "Vec<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "Map" if ty.args.len() == 2 => format!(
                "std::collections::HashMap<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "Set" if ty.args.len() == 1 => {
                format!(
                    "std::collections::HashSet<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "ResourcePool" if ty.args.len() == 1 => format!(
                "rsscript_runtime::ResourcePool<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            _ => {
                let name = rust_ident(&ty.name);
                if ty.args.is_empty() {
                    name
                } else {
                    format!(
                        "{}<{}>",
                        name,
                        ty.args
                            .iter()
                            .map(|arg| self.lower_type_ref(arg, ManagedPosition::Nested))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        };

        if self.should_wrap_in_gc(ty, position) {
            format!("rsscript_runtime::Gc<{lowered}>")
        } else {
            lowered
        }
    }

    fn should_wrap_in_gc(&self, ty: &TypeRef, position: ManagedPosition) -> bool {
        if !matches!(
            position,
            ManagedPosition::Field
                | ManagedPosition::Param
                | ManagedPosition::Return
                | ManagedPosition::Nested
        ) {
            return false;
        }
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Class))
    }
}

fn collect_mutated_bindings(block: &Block) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_mutated_bindings_from_block(block, &mut names);
    names
}

fn collect_mutated_bindings_from_block(block: &Block, names: &mut BTreeSet<String>) {
    for statement in &block.statements {
        collect_mutated_bindings_from_stmt(statement, names);
    }
}

fn collect_mutated_bindings_from_stmt(statement: &Stmt, names: &mut BTreeSet<String>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_mutated_bindings_from_expr(value, names);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_mutated_bindings_from_expr(value, names);
            }
        }
        Stmt::With(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.resource, names);
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::If(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.condition, names);
            collect_mutated_bindings_from_block(&stmt.then_body, names);
            if let Some(else_body) = &stmt.else_body {
                collect_mutated_bindings_from_block(else_body, names);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_mutated_bindings_from_expr(condition, names);
            }
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::Expr(expr) => collect_mutated_bindings_from_expr(expr, names),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
    }
}

fn collect_mutated_bindings_from_expr(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_mutated_bindings_from_expr(left, names);
            collect_mutated_bindings_from_expr(right, names);
        }
        Expr::Field { base, .. } => collect_mutated_bindings_from_expr(base, names),
        Expr::Index { base, index, .. } => {
            collect_mutated_bindings_from_expr(base, names);
            collect_mutated_bindings_from_expr(index, names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_mutated_bindings_from_expr(&arg.value, names);
            }
        }
        Expr::Effect { effect, value, .. } => {
            if *effect == DataEffect::Mut
                && let Some(name) = mutable_root_ident(value)
            {
                names.insert(name.to_string());
            }
            collect_mutated_bindings_from_expr(value, names);
        }
        Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            collect_mutated_bindings_from_expr(value, names);
        }
        Expr::Closure { body, .. } => collect_mutated_bindings_from_block(body, names),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn mutable_root_ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => mutable_root_ident(base),
        _ => None,
    }
}

fn stmt_span(statement: &Stmt) -> &Span {
    match statement {
        Stmt::Let(stmt) => &stmt.span,
        Stmt::Return(stmt) => &stmt.span,
        Stmt::With(stmt) => &stmt.span,
        Stmt::If(stmt) => &stmt.span,
        Stmt::Loop(stmt) => &stmt.span,
        Stmt::Break(span) | Stmt::Continue(span) | Stmt::Unknown(span) => span,
        Stmt::Expr(expr) => expr.span(),
    }
}

fn best_source_map_entry<'a>(
    source_map: &'a [RustSourceMapEntry],
    file: &str,
    line: usize,
    column: usize,
) -> Option<&'a RustSourceMapEntry> {
    source_map
        .iter()
        .filter(|entry| entry.generated.file == file)
        .filter(|entry| generated_span_starts_before_or_at(&entry.generated, line, column))
        .max_by_key(|entry| (entry.generated.line, entry.generated.column))
}

fn generated_span_starts_before_or_at(span: &Span, line: usize, column: usize) -> bool {
    span.line < line || (span.line == line && span.column <= column)
}

fn rustc_severity(level: &str) -> Severity {
    if level == "warning" {
        Severity::Warning
    } else {
        Severity::Error
    }
}

fn rustc_diagnostic_value(value: &serde_json::Value) -> Option<&serde_json::Value> {
    if value.get("level").is_some() && value.get("message").is_some() {
        return Some(value);
    }
    if value.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-message") {
        return value.get("message");
    }
    None
}

fn generated_span_from_rustc(span: &RustcJsonSpan) -> Span {
    Span {
        file: span.file_name.clone(),
        line: span.line_start,
        column: span.column_start,
        length: span.column_end.saturating_sub(span.column_start).max(1),
    }
}

#[derive(serde::Deserialize)]
struct RustcJsonDiagnostic {
    message: String,
    level: String,
    code: Option<RustcJsonCode>,
    #[serde(default)]
    spans: Vec<RustcJsonSpan>,
}

#[derive(serde::Deserialize)]
struct RustcJsonCode {
    code: String,
}

#[derive(serde::Deserialize)]
struct RustcJsonSpan {
    file_name: String,
    line_start: usize,
    column_start: usize,
    column_end: usize,
    #[serde(default)]
    is_primary: bool,
}

fn push_source_marker(
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
    }
}

fn generated_span_at_end(out: &str, file: &str, text: &str) -> Span {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedPosition {
    Field,
    Param,
    Return,
    FreshReturn,
    Nested,
}

fn visibility(is_public: bool) -> &'static str {
    if is_public { "pub " } else { "" }
}

fn lower_generic_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }

    let params = params
        .iter()
        .map(|param| {
            let name = rust_ident(&param.name);
            match param.bound {
                Some(GenericBound::Managed) => format!("{name}: rsscript_runtime::Managed"),
                Some(GenericBound::Struct) => name,
                Some(GenericBound::Resource) => format!("{name}: rsscript_runtime::Resource"),
                None => name,
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{params}>")
}

fn lower_impl_generics(params: &[GenericParam]) -> String {
    lower_generic_params(params)
}

fn lower_generic_args(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }

    let args = params
        .iter()
        .map(|param| rust_ident(&param.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{args}>")
}

fn is_native_boundary(effect: &EffectDecl) -> bool {
    matches!(effect, EffectDecl::Name(name) if matches!(name.as_str(), "native" | "unsafe"))
}

fn lower_callee(callee: &Callee) -> String {
    match callee {
        callee if is_log_write_callee(callee) => "rsscript_runtime::log_write".to_string(),
        callee if is_assert_equal_callee(callee) => "rsscript_runtime::assert_equal".to_string(),
        callee if is_file_open_callee(callee) => "rsscript_runtime::file_open".to_string(),
        callee if is_file_open_read_callee(callee) => {
            "rsscript_runtime::file_open_read".to_string()
        }
        callee if is_file_open_write_callee(callee) => {
            "rsscript_runtime::file_open_write".to_string()
        }
        callee if is_file_read_all_callee(callee) => "rsscript_runtime::file_read_all".to_string(),
        callee if is_file_write_callee(callee) => "rsscript_runtime::file_write".to_string(),
        callee if is_request_new_callee(callee) => "rsscript_runtime::request_new".to_string(),
        callee if is_request_path_callee(callee) => "rsscript_runtime::request_path".to_string(),
        callee if is_response_ok_callee(callee) => "rsscript_runtime::response_ok".to_string(),
        callee if is_response_status_callee(callee) => {
            "rsscript_runtime::response_status".to_string()
        }
        callee if is_response_body_callee(callee) => "rsscript_runtime::response_body".to_string(),
        callee if is_config_load_callee(callee) => "rsscript_runtime::config_load".to_string(),
        callee if is_config_name_callee(callee) => "rsscript_runtime::config_name".to_string(),
        callee if is_config_store_new_callee(callee) => {
            "rsscript_runtime::config_store_new".to_string()
        }
        callee if is_config_store_replace_callee(callee) => {
            "rsscript_runtime::config_store_replace".to_string()
        }
        callee if is_config_store_name_callee(callee) => {
            "rsscript_runtime::config_store_name".to_string()
        }
        callee if is_counter_new_callee(callee) => "rsscript_runtime::counter_new".to_string(),
        callee if is_counter_add_callee(callee) => "rsscript_runtime::counter_add".to_string(),
        callee if is_counter_value_callee(callee) => "rsscript_runtime::counter_value".to_string(),
        callee if is_db_connection_open_callee(callee) => {
            "rsscript_runtime::db_connection_open".to_string()
        }
        callee if is_db_connection_query_callee(callee) => {
            "rsscript_runtime::db_connection_query".to_string()
        }
        callee if is_db_close_callee(callee) => "rsscript_runtime::db_close".to_string(),
        callee if is_image_load_callee(callee) => "rsscript_runtime::image_load".to_string(),
        callee if is_image_resize_callee(callee) => "rsscript_runtime::image_resize".to_string(),
        callee if is_image_normalize_callee(callee) => {
            "rsscript_runtime::image_normalize".to_string()
        }
        callee if is_image_sharpen_callee(callee) => "rsscript_runtime::image_sharpen".to_string(),
        callee if is_image_save_callee(callee) => "rsscript_runtime::image_save".to_string(),
        callee if is_image_inspect_callee(callee) => "rsscript_runtime::image_inspect".to_string(),
        callee if is_json_parse_callee(callee) => "rsscript_runtime::json_parse".to_string(),
        callee if is_json_field_callee(callee) => "rsscript_runtime::json_field".to_string(),
        callee if is_json_field_string_callee(callee) => {
            "rsscript_runtime::json_field_string".to_string()
        }
        callee if is_row_buffer_new_callee(callee) => {
            "rsscript_runtime::row_buffer_new".to_string()
        }
        callee if is_csv_read_into_callee(callee) => "rsscript_runtime::csv_read_into".to_string(),
        callee if is_csv_parse_row_callee(callee) => "rsscript_runtime::csv_parse_row".to_string(),
        callee if is_row_field_string_callee(callee) => {
            "rsscript_runtime::row_field_string".to_string()
        }
        Callee::Name(name) => rust_ident(name),
        Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" => {
            format!("rsscript_runtime::ResourcePool::{}", rust_ident(name))
        }
        Callee::Qualified { namespace, name } => {
            format!("{}::{}", lower_namespace(namespace), rust_ident(name))
        }
    }
}

fn is_log_write_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Log" && name == "write")
}

fn is_assert_equal_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Assert" && name == "equal")
}

fn is_string_concat_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "String" && name == "concat")
}

fn is_file_open_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && name == "open")
}

fn is_file_open_read_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && name == "open_read")
}

fn is_file_open_write_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && name == "open_write")
}

fn is_file_read_all_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && name == "read_all")
}

fn is_file_write_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && name == "write")
}

fn is_request_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Request" && name == "new")
}

fn is_request_path_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Request" && name == "path")
}

fn is_response_ok_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Response" && name == "ok")
}

fn is_response_status_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Response" && name == "status")
}

fn is_response_body_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Response" && name == "body")
}

fn is_config_load_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Config" && name == "load")
}

fn is_config_name_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Config" && name == "name")
}

fn is_config_store_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ConfigStore" && name == "new")
}

fn is_config_store_replace_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ConfigStore" && name == "replace")
}

fn is_config_store_name_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ConfigStore" && name == "name")
}

fn is_counter_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Counter" && name == "new")
}

fn is_counter_add_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Counter" && name == "add")
}

fn is_counter_value_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Counter" && name == "value")
}

fn is_db_connection_open_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "DbConnection" && name == "open")
}

fn is_db_connection_query_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "DbConnection" && name == "query")
}

fn is_db_close_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Db" && name == "close")
}

fn is_image_load_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Image" && name == "load")
}

fn is_image_resize_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Image" && name == "resize")
}

fn is_image_normalize_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Image" && name == "normalize")
}

fn is_image_sharpen_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Image" && name == "sharpen")
}

fn is_image_save_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Image" && name == "save")
}

fn is_image_inspect_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Image" && name == "inspect")
}

fn is_json_parse_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Json" && name == "parse")
}

fn is_json_field_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Json" && name == "field")
}

fn is_json_field_string_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Json" && name == "field_string")
}

fn is_row_buffer_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "RowBuffer" && name == "new")
}

fn is_csv_read_into_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Csv" && name == "read_into")
}

fn is_csv_parse_row_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Csv" && name == "parse_row")
}

fn is_row_field_string_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "Row" && name == "field_string")
}

fn is_file_open_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if is_file_open_callee(callee) || is_file_open_read_callee(callee) || is_file_open_write_callee(callee))
}

fn lower_string_concat_call(lowerer: &mut RustLowerer<'_>, args: &[CallArg]) -> String {
    let left = lower_call_arg(lowerer, args, "left", 0, "\"\".to_string()");
    let right = lower_call_arg(lowerer, args, "right", 1, "\"\".to_string()");
    format!("format!(\"{{}}{{}}\", {left}, {right})")
}

fn lower_call_arg(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    name: &str,
    index: usize,
    default: &str,
) -> String {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .or_else(|| args.get(index))
        .map(|arg| lowerer.lower_expr(&arg.value))
        .unwrap_or_else(|| default.to_string())
}

fn is_resource_pool_borrow_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && name == "borrow")
}

fn is_resource_pool_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && name == "new")
}

fn is_resource_pool_borrow_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if is_resource_pool_borrow_callee(callee))
}

fn lower_resource_pool_new_call(lowerer: &mut RustLowerer<'_>, args: &[CallArg]) -> String {
    let create = lower_call_arg(
        lowerer,
        args,
        "create",
        0,
        "|| panic!(\"missing resource factory\")",
    );
    let max_size = lower_call_arg(lowerer, args, "max_size", 1, "0");
    format!("rsscript_runtime::ResourcePool::from_factory({max_size}, {create})")
}

fn lower_builtin_value_ident(name: &str) -> Option<&'static str> {
    match name {
        "Unit" => Some("()"),
        "true" => Some("true"),
        "false" => Some("false"),
        "None" => Some("None"),
        _ => None,
    }
}

fn decode_string_token(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some('0') => decoded.push('\0'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

fn is_rust_enum_constructor(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some")
}

fn lower_source_span(span: &Span) -> String {
    format!(
        "rsscript_runtime::SourceSpan::new({:?}, {}, {}, {})",
        span.file, span.line, span.column, span.length
    )
}

fn lower_namespace(namespace: &str) -> String {
    let namespace = namespace.replace('<', "::<");
    namespace
        .split('.')
        .map(rust_path_segment)
        .collect::<Vec<_>>()
        .join("::")
}

fn type_root_name(name: &str) -> &str {
    name.split('<').next().unwrap_or(name)
}

fn rust_path_segment(segment: &str) -> String {
    if let Some((head, tail)) = segment.split_once("::<") {
        format!("{}::<{}", rust_ident(head), tail)
    } else {
        rust_ident(segment)
    }
}

fn rust_ident(name: &str) -> String {
    let keywords: BTreeSet<&'static str> = [
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ]
    .into_iter()
    .collect();

    if keywords.contains(name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

fn cargo_package_name(name: &str) -> String {
    let mut out = String::new();
    let mut previous_was_dash = false;
    let mut previous_was_lower_or_digit = false;

    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase()
                && previous_was_lower_or_digit
                && !previous_was_dash
                && !out.is_empty()
            {
                out.push('-');
            }
            out.push(character.to_ascii_lowercase());
            previous_was_dash = false;
            previous_was_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
        } else if (character.is_ascii_whitespace() || matches!(character, '-' | '_' | '.'))
            && !out.is_empty()
            && !previous_was_dash
        {
            out.push('-');
            previous_was_dash = true;
            previous_was_lower_or_digit = false;
        } else {
            previous_was_lower_or_digit = false;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "rsscript-generated".to_string()
    } else {
        out
    }
}

fn rust_package_main(program: &Program, package_name: &str) -> Option<String> {
    let main = program.items.iter().find_map(|item| match item {
        Item::Function(function) => runnable_main_kind(function).map(|kind| (function, kind)),
        Item::Type(_) => None,
    })?;
    let (main, kind) = main;
    let crate_name = cargo_crate_name(package_name);
    let call = match kind {
        RunnableMainKind::Unit => format!("{}::{}();", crate_name, rust_ident(&main.name)),
        RunnableMainKind::ResultUnit => format!(
            "{}::{}().expect(\"RSScript main returned an error\");",
            crate_name,
            rust_ident(&main.name)
        ),
    };
    Some(format!(
        concat!(
            "// Generated by RSScript. Edit the .rss source instead.\n",
            "// Runnable harness for RSScript `{}`.\n\n",
            "fn main() {{\n",
            "    {}\n",
            "}}\n"
        ),
        main.name, call
    ))
}

fn is_runnable_main(function: &FunctionDecl) -> bool {
    runnable_main_kind(function).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunnableMainKind {
    Unit,
    ResultUnit,
}

fn runnable_main_kind(function: &FunctionDecl) -> Option<RunnableMainKind> {
    if function.name != "main" || !function.params.is_empty() {
        return None;
    }
    let Some(return_ty) = function.return_ty.as_ref() else {
        return Some(RunnableMainKind::Unit);
    };
    if return_ty.name == "Unit" && return_ty.args.is_empty() {
        return Some(RunnableMainKind::Unit);
    }
    if return_ty.name == "Result"
        && return_ty.args.len() == 2
        && return_ty.args[0].name == "Unit"
        && return_ty.args[0].args.is_empty()
    {
        return Some(RunnableMainKind::ResultUnit);
    }
    None
}

fn cargo_crate_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
