use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::analyzer::{analyze_source_with_core, analyze_sources_with_interfaces};
use crate::diagnostic::{Diagnostic, Severity, Span, code};
use crate::interfaces::builtin_interfaces;
use crate::runtime_abi;
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature,
    FunctionDecl, GenericBound, GenericParam, Item, MatchPattern, Param, Program, Stmt, TypeDecl,
    TypeKind, TypeRef, merge_programs,
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
pub struct NativeRustDependency {
    pub crate_name: String,
    pub path: String,
    pub bindings: BTreeMap<String, String>,
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

const RUNTIME_DIAGNOSTIC_PREFIX: &str = "RSSCRIPT_RUNTIME_DIAGNOSTIC:";

#[derive(serde::Deserialize)]
struct RuntimeDiagnosticJson {
    code: Option<String>,
    severity: Option<String>,
    summary: String,
    file: String,
    line: usize,
    column: usize,
    length: usize,
    label: String,
    kind: Option<String>,
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
    let lowering_diagnostics = validate_executable_declarations(&program);
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    Ok(lower_program_to_rust_with_map(&program))
}

pub fn lower_source_to_rust_package(
    file: &str,
    source: &str,
    package_name: &str,
    runtime_path: &str,
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    lower_source_to_rust_package_with_interfaces(file, source, package_name, runtime_path, &[])
}

pub fn lower_source_to_rust_package_with_interfaces(
    file: &str,
    source: &str,
    package_name: &str,
    runtime_path: &str,
    interfaces: &[(String, String)],
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    lower_sources_to_rust_package_with_interfaces(
        &[(file.to_string(), source.to_string())],
        package_name,
        runtime_path,
        interfaces,
    )
}

pub fn lower_sources_to_rust_package_with_interfaces(
    sources: &[(String, String)],
    package_name: &str,
    runtime_path: &str,
    interfaces: &[(String, String)],
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    lower_sources_to_rust_package_with_options(sources, package_name, runtime_path, interfaces, &[])
}

pub fn lower_sources_to_rust_package_with_options(
    sources: &[(String, String)],
    package_name: &str,
    runtime_path: &str,
    interfaces: &[(String, String)],
    native_dependencies: &[NativeRustDependency],
) -> Result<GeneratedRustPackage, Vec<Diagnostic>> {
    let mut interface_refs = builtin_interfaces().collect::<Vec<_>>();
    interface_refs.extend(
        interfaces
            .iter()
            .map(|(path, contents)| (path.as_str(), contents.as_str())),
    );
    let source_refs = sources
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let diagnostics = analyze_sources_with_interfaces(&source_refs, &interface_refs);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return Err(diagnostics);
    }

    let program = merge_programs(
        sources
            .iter()
            .map(|(path, source)| parse_source(path, source)),
    );
    let lowering_diagnostics = validate_executable_declarations(&program);
    if !lowering_diagnostics.is_empty() {
        return Err(lowering_diagnostics);
    }
    let native_bindings = native_dependencies
        .iter()
        .flat_map(|dependency| dependency.bindings.iter())
        .map(|(symbol, target)| (symbol.clone(), target.clone()))
        .collect::<BTreeMap<_, _>>();
    let lowered = lower_program_to_rust_with_map_with_native_bindings(&program, native_bindings);
    let package_name = cargo_package_name(package_name);
    let native_dependency_toml = native_dependencies
        .iter()
        .map(|dependency| {
            format!(
                "\"{}\" = {{ path = \"{}\" }}\n",
                toml_string(&dependency.crate_name),
                toml_string(&dependency.path)
            )
        })
        .collect::<String>();
    let cargo_toml = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nrsscript-runtime = {{ path = \"{}\" }}\n{native_dependency_toml}",
        toml_string(runtime_path),
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

pub fn parse_runtime_diagnostics(stderr: &str) -> Vec<Diagnostic> {
    stderr
        .lines()
        .filter_map(parse_runtime_diagnostic_line)
        .collect()
}

fn parse_runtime_diagnostic_line(line: &str) -> Option<Diagnostic> {
    let start = line.find(RUNTIME_DIAGNOSTIC_PREFIX)? + RUNTIME_DIAGNOSTIC_PREFIX.len();
    let payload = &line[start..];
    let wire: RuntimeDiagnosticJson = serde_json::from_str(payload).ok()?;
    let code = wire
        .code
        .unwrap_or_else(|| code::RUNTIME_DIAGNOSTIC.to_string());
    let span = Span {
        file: wire.file,
        line: wire.line,
        column: wire.column,
        length: wire.length,
    };
    let mut diagnostic = match wire.severity.as_deref() {
        Some("warning") => Diagnostic::warning(&code, wire.summary, span, wire.label),
        _ => Diagnostic::error(&code, wire.summary, span, wire.label),
    };
    if let Some(kind) = wire.kind {
        diagnostic = diagnostic.with_cause(format!("runtime error kind: {kind}"));
    }
    Some(diagnostic)
}

pub fn lower_program_to_rust(program: &Program) -> String {
    lower_program_to_rust_with_map(program).rust_source
}

pub fn lower_program_to_rust_with_map(program: &Program) -> LoweredRust {
    lower_program_to_rust_with_map_with_native_bindings(program, BTreeMap::new())
}

fn lower_program_to_rust_with_map_with_native_bindings(
    program: &Program,
    native_bindings: BTreeMap<String, String>,
) -> LoweredRust {
    RustLowerer::new(program, native_bindings).lower()
}

fn validate_executable_declarations(program: &Program) -> Vec<Diagnostic> {
    let mut implemented = BTreeSet::new();
    let mut bodyless = BTreeMap::new();
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let key = executable_declaration_function_key(&function.name);
        if function.body.statements.is_empty() {
            if !function.is_native {
                bodyless.insert(key, function.name.clone());
            }
        } else {
            implemented.insert(key);
        }
    }
    for key in &implemented {
        bodyless.remove(key);
    }
    if bodyless.is_empty() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if function.body.statements.is_empty() {
            continue;
        }
        validate_executable_declarations_in_block(&function.body, &bodyless, &mut diagnostics);
    }
    diagnostics
}

fn validate_executable_declarations_in_block(
    block: &Block,
    bodyless: &BTreeMap<String, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        validate_executable_declarations_in_stmt(statement, bodyless, diagnostics);
    }
}

fn validate_executable_declarations_in_stmt(
    statement: &Stmt,
    bodyless: &BTreeMap<String, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                validate_executable_declarations_in_expr(value, bodyless, diagnostics);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                validate_executable_declarations_in_expr(value, bodyless, diagnostics);
            }
        }
        Stmt::With(stmt) => {
            validate_executable_declarations_in_expr(&stmt.resource, bodyless, diagnostics);
            validate_executable_declarations_in_block(&stmt.body, bodyless, diagnostics);
        }
        Stmt::If(stmt) => {
            validate_executable_declarations_in_expr(&stmt.condition, bodyless, diagnostics);
            validate_executable_declarations_in_block(&stmt.then_body, bodyless, diagnostics);
            if let Some(else_body) = &stmt.else_body {
                validate_executable_declarations_in_block(else_body, bodyless, diagnostics);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                validate_executable_declarations_in_expr(condition, bodyless, diagnostics);
            }
            validate_executable_declarations_in_block(&stmt.body, bodyless, diagnostics);
        }
        Stmt::For(stmt) => {
            validate_executable_declarations_in_expr(&stmt.iterable, bodyless, diagnostics);
            validate_executable_declarations_in_block(&stmt.body, bodyless, diagnostics);
        }
        Stmt::Match(stmt) => {
            validate_executable_declarations_in_expr(&stmt.value, bodyless, diagnostics);
            for arm in &stmt.arms {
                validate_executable_declarations_in_block(&arm.body, bodyless, diagnostics);
            }
        }
        Stmt::Expr(expr) => validate_executable_declarations_in_expr(expr, bodyless, diagnostics),
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
    bodyless: &BTreeMap<String, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            let key = executable_declaration_callee_key(callee);
            if runtime_intrinsic_target(callee).is_none()
                && let Some(function_name) = bodyless.get(&key)
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
            for arg in args {
                validate_executable_declarations_in_expr(&arg.value, bodyless, diagnostics);
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_executable_declarations_in_expr(left, bodyless, diagnostics);
            validate_executable_declarations_in_expr(right, bodyless, diagnostics);
        }
        Expr::Field { base, .. } => {
            validate_executable_declarations_in_expr(base, bodyless, diagnostics);
        }
        Expr::Index { base, index, .. } => {
            validate_executable_declarations_in_expr(base, bodyless, diagnostics);
            validate_executable_declarations_in_expr(index, bodyless, diagnostics);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            validate_executable_declarations_in_expr(value, bodyless, diagnostics);
        }
        Expr::Closure { body, .. } => {
            validate_executable_declarations_in_block(body, bodyless, diagnostics);
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn executable_declaration_function_key(name: &str) -> String {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        format!("{}.{}", type_root_name(namespace), name)
    } else {
        name.to_string()
    }
}

fn executable_declaration_callee_key(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{}.{}", type_root_name(namespace), name),
    }
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
    native_boundary_callees: BTreeSet<String>,
    native_bindings: BTreeMap<String, String>,
    function_return_types: BTreeMap<String, TypeRef>,
    retained_params_by_callee: BTreeMap<String, BTreeSet<String>>,
    param_effects: BTreeMap<String, DataEffect>,
    value_types: BTreeMap<String, TypeRef>,
    managed_bindings: BTreeSet<String>,
    current_retained_params: BTreeSet<String>,
    mutated_bindings: BTreeSet<String>,
    drop_field_names: BTreeSet<String>,
    current_return_type: Option<TypeRef>,
    source_map: Vec<RustSourceMapEntry>,
}

impl<'a> RustLowerer<'a> {
    fn new(program: &'a Program, native_bindings: BTreeMap<String, String>) -> Self {
        let type_kinds = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Type(ty) => Some((ty.name.clone(), ty.kind)),
                Item::Function(_) => None,
            })
            .collect();
        let native_boundary_callees = collect_native_boundary_callees(program);
        let function_return_types = collect_function_return_types(program);
        let retained_params_by_callee = collect_function_retained_params(program);

        Self {
            program,
            type_kinds,
            native_boundary_callees,
            native_bindings,
            function_return_types,
            retained_params_by_callee,
            param_effects: BTreeMap::new(),
            value_types: BTreeMap::new(),
            managed_bindings: BTreeSet::new(),
            current_retained_params: BTreeSet::new(),
            mutated_bindings: BTreeSet::new(),
            drop_field_names: BTreeSet::new(),
            current_return_type: None,
            source_map: Vec::new(),
        }
    }

    fn lower(mut self) -> LoweredRust {
        let mut out = String::new();
        out.push_str("// Generated by RSScript. Edit the .rss source instead.\n");
        out.push_str(
            "// Runtime hooks are intentionally explicit while Rust lowering is stabilizing.\n",
        );
        out.push_str("#![allow(dead_code, non_snake_case)]\n");
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

        for (index, item) in self.program.items.iter().enumerate() {
            match item {
                Item::Type(ty) => self.lower_type_decl(ty, &mut out),
                Item::Function(function) if !function.body.statements.is_empty() => {
                    self.lower_function(function, &mut out)
                }
                Item::Function(_) => {}
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
        if ty.kind == TypeKind::Resource {
            out.push_str("#[derive(Debug)]\n");
        } else {
            out.push_str("#[derive(Debug, Clone)]\n");
        }
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
            if let Some(drop_body) = &ty.drop_body {
                let previous_drop_field_names = std::mem::take(&mut self.drop_field_names);
                self.drop_field_names = ty.fields.iter().map(|field| field.name.clone()).collect();
                self.lower_block(drop_body, out, 2);
                self.drop_field_names = previous_drop_field_names;
            } else {
                out.push_str("        // RSScript resource has no explicit drop body.\n");
            }
            out.push_str("    }\n");
            out.push_str("}\n");
        }
    }

    fn lower_field_decl(&mut self, field: &FieldDecl, out: &mut String) {
        let rust_ty = self.lower_type_ref(&field.ty, ManagedPosition::Bare);
        self.source_map.push(RustSourceMapEntry {
            kind: "field".to_string(),
            source: field.span.clone(),
            generated: generated_span_at_end(out, "src/lib.rs", "field"),
        });
        if field.is_weak {
            out.push_str(&format!(
                "    pub {}: rsscript_runtime::WeakManaged<{}>,\n",
                rust_ident(&field.name),
                rust_ty
            ));
        } else if field.is_handle
            || matches!(self.type_kinds.get(&field.ty.name), Some(TypeKind::Class))
        {
            out.push_str(&format!(
                "    pub {}: rsscript_runtime::Managed<{}>,\n",
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
        let previous_value_types = std::mem::take(&mut self.value_types);
        let previous_managed_bindings = std::mem::take(&mut self.managed_bindings);
        let previous_retained_params = std::mem::take(&mut self.current_retained_params);
        let previous_mutated_bindings = std::mem::take(&mut self.mutated_bindings);
        let previous_return_type = self.current_return_type.take();
        self.param_effects = function
            .params
            .iter()
            .filter_map(|param| param.effect.map(|effect| (param.name.clone(), effect)))
            .collect();
        self.value_types = function
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect();
        self.managed_bindings = function
            .params
            .iter()
            .filter(|param| self.is_class_type(&param.ty))
            .map(|param| param.name.clone())
            .collect();
        self.current_retained_params = function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Retains(param) => Some(param.clone()),
                EffectDecl::Name(_) => None,
            })
            .collect();
        self.mutated_bindings = collect_mutated_bindings(&function.body);
        self.current_return_type = function.return_ty.clone();
        self.record_source_marker(out, 0, "function", &function.span);
        let async_prefix = if function.is_async { "async " } else { "" };
        let is_public = function.is_public || is_runnable_main(function);
        out.push_str(&format!(
            "{}{}fn {}{}(",
            visibility(is_public),
            async_prefix,
            rust_function_ident(&function.name),
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
        self.lower_block(&function.body, out, 1);
        out.push_str("}\n");
        self.param_effects = previous_param_effects;
        self.value_types = previous_value_types;
        self.managed_bindings = previous_managed_bindings;
        self.current_retained_params = previous_retained_params;
        self.mutated_bindings = previous_mutated_bindings;
        self.current_return_type = previous_return_type;
    }

    fn lower_param(&self, param: &Param) -> String {
        let ty = if param.effect == Some(DataEffect::Read)
            && self.current_retained_params.contains(&param.name)
            && !self.is_class_type(&param.ty)
            && self.type_kinds.contains_key(&param.ty.name)
        {
            format!(
                "rsscript_runtime::Managed<{}>",
                self.lower_type_ref(&param.ty, ManagedPosition::Bare)
            )
        } else {
            self.lower_type_ref(&param.ty, ManagedPosition::Param)
        };
        let rust_ty = match param.effect {
            Some(DataEffect::Read) => format!("&{ty}"),
            Some(DataEffect::Mut) if self.is_class_type(&param.ty) => format!("&{ty}"),
            Some(DataEffect::Mut) => format!("&mut {ty}"),
            Some(DataEffect::Take) | None => ty,
        };
        let name = rust_ident(&param.name);
        if param.ty.is_noescape && param.ty.name == "Fn" {
            format!("mut {name}: {rust_ty}")
        } else {
            format!("{name}: {rust_ty}")
        }
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
                    let lowered = self.lower_expr(value);
                    let inferred_ty = self.infer_expr_type(value);
                    out.push_str(&format!(
                        "{pad}let {mutable}{} = {};\n",
                        rust_ident(&stmt.name),
                        lowered
                    ));
                    if let Some(ty) = inferred_ty {
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
            Stmt::For(stmt) => {
                let iterable = self.lower_expr(&stmt.iterable);
                let previous_type = self.value_types.get(&stmt.binding).cloned();
                let previous_managed = self.managed_bindings.contains(&stmt.binding);
                if let Some(item_type) = self
                    .infer_expr_type(&stmt.iterable)
                    .as_ref()
                    .and_then(list_element_type_ref)
                {
                    if self.is_class_type(&item_type) {
                        self.managed_bindings.insert(stmt.binding.clone());
                    } else {
                        self.managed_bindings.remove(&stmt.binding);
                    }
                    self.value_types.insert(stmt.binding.clone(), item_type);
                }
                out.push_str(&format!(
                    "{pad}for {} in ({iterable}).iter().cloned() {{\n",
                    rust_ident(&stmt.binding)
                ));
                self.lower_block(&stmt.body, out, indent + 1);
                out.push_str(&format!("{pad}}}\n"));
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
            }
            Stmt::Match(stmt) => {
                out.push_str(&format!("{pad}match {} {{\n", self.lower_expr(&stmt.value)));
                for arm in &stmt.arms {
                    out.push_str(&format!(
                        "{}{} => {{\n",
                        "    ".repeat(indent + 1),
                        lower_match_pattern(&arm.pattern)
                    ));
                    self.lower_block(&arm.body, out, indent + 2);
                    out.push_str(&format!("{}}},\n", "    ".repeat(indent + 1)));
                }
                out.push_str(&format!("{pad}}}\n"));
            }
            Stmt::Break(_) => out.push_str(&format!("{pad}break;\n")),
            Stmt::Continue(_) => out.push_str(&format!("{pad}continue;\n")),
            Stmt::Expr(expr) => out.push_str(&format!("{pad}{};\n", self.lower_expr(expr))),
            Stmt::MalformedWith(span)
            | Stmt::MalformedIf(span)
            | Stmt::MalformedLoop(span)
            | Stmt::MalformedFor(span)
            | Stmt::MalformedMatch(span)
            | Stmt::Unknown(span) => unreachable_lowering("statement", span),
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
            Stmt::For(stmt) => {
                self.record_expr_source_map(&stmt.iterable, generated);
                self.record_block_source_map(&stmt.body, generated);
            }
            Stmt::Match(stmt) => {
                self.record_expr_source_map(&stmt.value, generated);
                for arm in &stmt.arms {
                    self.record_block_source_map(&arm.body, generated);
                }
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
            Expr::Call { callee, args, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "call".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                });
                if self.is_native_boundary_call(callee) {
                    self.source_map.push(RustSourceMapEntry {
                        kind: "native_call".to_string(),
                        source: span.clone(),
                        generated: generated.clone(),
                    });
                }
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
            Expr::Spawn { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "spawn".to_string(),
                    source: span.clone(),
                    generated: generated.clone(),
                });
                self.record_expr_source_map(value, generated);
            }
            Expr::Await { value, span } => {
                self.source_map.push(RustSourceMapEntry {
                    kind: "await".to_string(),
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

    fn is_native_boundary_call(&self, callee: &Callee) -> bool {
        let key = native_boundary_callee_key(callee);
        self.native_boundary_callees.contains(&key) || self.native_bindings.contains_key(&key)
    }

    fn lower_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(name, _) => {
                if self.drop_field_names.contains(name) {
                    format!("self.{}", rust_ident(name))
                } else {
                    lower_builtin_value_ident(name)
                        .map(str::to_string)
                        .unwrap_or_else(|| rust_ident(name))
                }
            }
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
                format!("{} {op} {}", self.lower_expr(left), self.lower_expr(right))
            }
            Expr::Field { base, name, span } => {
                if self
                    .infer_expr_type(base)
                    .is_some_and(|ty| self.is_class_type(&ty))
                {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_read_at({})).{}.clone()",
                        self.lower_expr(base),
                        lower_source_span(span),
                        rust_ident(name)
                    )
                } else {
                    format!("{}.{}", self.lower_expr(base), rust_ident(name))
                }
            }
            Expr::Index { base, index, .. } => {
                format!("{}[{}]", self.lower_expr(base), self.lower_expr(index))
            }
            Expr::Call { callee, args, span } => {
                if let Callee::Name(name) = callee {
                    if let Some(type_kind) = self.type_kinds.get(name).copied() {
                        let mut fields = Vec::new();
                        for arg in args {
                            let field = arg
                                .name
                                .as_deref()
                                .map(rust_ident)
                                .unwrap_or_else(|| "/* unnamed */".to_string());
                            let is_weak_field = arg
                                .name
                                .as_deref()
                                .is_some_and(|field_name| self.is_weak_field(name, field_name));
                            if is_weak_field {
                                fields.push(format!(
                                    "{field}: {}",
                                    self.lower_explicit_weak_field_value(&arg.value)
                                ));
                            } else {
                                let value = self.lower_owned_expr(&arg.value);
                                fields.push(format!("{field}: {value}"));
                            }
                        }
                        let fields = fields.join(", ");
                        let constructed = format!("{} {{ {fields} }}", rust_ident(name));
                        if type_kind == TypeKind::Class {
                            return format!(
                                "rsscript_runtime::manage_at({constructed}, {})",
                                lower_source_span(span)
                            );
                        }
                        return constructed;
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
                let is_resource_pool_borrow = is_resource_pool_borrow_callee(callee);
                let lowered_callee = if is_resource_pool_borrow {
                    "rsscript_runtime::ResourcePool::borrow_at".to_string()
                } else {
                    lower_callee(callee)
                };
                let mut args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| self.lower_call_arg_for_callee(callee, arg, index))
                    .collect::<Vec<_>>();
                if is_resource_pool_borrow {
                    args.push(lower_source_span(span));
                }
                let args = args.join(", ");
                format!("{lowered_callee}({args})")
            }
            Expr::Effect {
                effect,
                value,
                span,
            } => match effect {
                DataEffect::Read => {
                    if self.expr_lowers_to_managed_non_class_handle(value) {
                        format!(
                            "&*rsscript_runtime::unwrap_runtime({}.try_read_at({}))",
                            self.lower_expr(value),
                            lower_source_span(span)
                        )
                    } else {
                        format!("&{}", self.lower_expr(value))
                    }
                }
                DataEffect::Mut => {
                    if let Expr::Ident(name, _) = &**value
                        && self.param_effects.get(name) == Some(&DataEffect::Mut)
                    {
                        rust_ident(name)
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
            Expr::Await { span, .. } => unreachable_lowering("await expression", span),
            Expr::Try { value, .. } => format!("{}?", self.lower_expr(value)),
            Expr::Closure { params, body, .. } => {
                let lowered_params = params
                    .iter()
                    .map(|param| rust_ident(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let [Stmt::Expr(value)] = body.statements.as_slice() {
                    return format!("|{lowered_params}| {}", self.lower_expr(value));
                }
                let mut out = String::new();
                out.push_str(&format!("|{lowered_params}| {{\n"));
                self.lower_block(body, &mut out, 1);
                out.push('}');
                out
            }
            Expr::Unknown(span) => unreachable_lowering("expression", span),
        }
    }

    fn lower_owned_expr(&mut self, expr: &Expr) -> String {
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

    fn lower_call_arg_for_callee(
        &mut self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> String {
        if (self.call_arg_is_retained(callee, arg, index)
            || runtime_intrinsic_wants_managed_handle_arg(callee, arg.name.as_deref()))
            && let Expr::Effect { effect, value, .. } = &arg.value
            && self.expr_lowers_to_managed_handle(value)
        {
            return match effect {
                DataEffect::Read => format!("&{}", self.lower_expr(value)),
                DataEffect::Mut => format!("&mut {}", self.lower_expr(value)),
                DataEffect::Take => self.lower_expr(value),
            };
        }
        self.lower_expr(&arg.value)
    }

    fn call_arg_is_retained(&self, callee: &Callee, arg: &CallArg, _index: usize) -> bool {
        let Some(name) = arg.name.as_deref() else {
            return false;
        };
        self.retained_params_by_callee
            .get(&native_boundary_callee_key(callee))
            .is_some_and(|retained| retained.contains(name))
    }

    fn lower_return_expr(&mut self, expr: &Expr) -> String {
        let lowered = self.lower_expr(expr);
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

    fn expr_returns_result(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Call { callee, .. } => self
                .function_return_types
                .get(&native_boundary_callee_key(callee))
                .is_some_and(is_result_type),
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
                self.expr_returns_result(value)
            }
            _ => false,
        }
    }

    fn infer_expr_type(&self, expr: &Expr) -> Option<TypeRef> {
        match expr {
            Expr::Ident(name, _) => self.value_types.get(name).cloned(),
            Expr::Call {
                callee: Callee::Name(name),
                span,
                ..
            } if self.type_kinds.contains_key(name) => Some(TypeRef {
                name: name.clone(),
                args: Vec::new(),
                malformed_arg_spans: Vec::new(),
                is_fresh: false,
                is_noescape: false,
                fn_params: Vec::new(),
                fn_return: None,
                span: span.clone(),
            }),
            Expr::Manage { value, .. } | Expr::Try { value, .. } => self.infer_expr_type(value),
            _ => None,
        }
    }

    fn expr_lowers_to_managed_handle(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => self.managed_bindings.contains(name),
            Expr::Manage { .. } => true,
            Expr::Call {
                callee: Callee::Name(name),
                ..
            } => self
                .type_kinds
                .get(name)
                .is_some_and(|kind| *kind == TypeKind::Class),
            Expr::Effect { value, .. } | Expr::Try { value, .. } => {
                self.expr_lowers_to_managed_handle(value)
            }
            _ => false,
        }
    }

    fn expr_lowers_to_managed_non_class_handle(&self, expr: &Expr) -> bool {
        self.expr_lowers_to_managed_handle(expr)
            && !self
                .infer_expr_type(expr)
                .is_some_and(|ty| self.is_class_type(&ty))
    }

    fn is_string_comparison_operand(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::String(_, _))
            || self
                .infer_expr_type(expr)
                .is_some_and(|ty| ty.name == "String" && ty.args.is_empty())
    }

    fn lower_string_comparison_operand(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::String(value, _) => format!("{:?}", decode_string_token(value)),
            _ if self.is_string_comparison_operand(expr) => {
                format!("{}.as_str()", self.lower_expr(expr))
            }
            _ => self.lower_expr(expr),
        }
    }

    fn lower_type_ref(&self, ty: &TypeRef, position: ManagedPosition) -> String {
        if ty.is_noescape && ty.name == "Fn" {
            let params = ty
                .fn_params
                .iter()
                .map(|param| self.lower_type_ref(param, ManagedPosition::Param))
                .collect::<Vec<_>>()
                .join(", ");
            let return_ty = ty.fn_return.as_ref().map(|return_ty| {
                format!(
                    " -> {}",
                    self.lower_type_ref(return_ty, ManagedPosition::Return)
                )
            });
            return match position {
                ManagedPosition::Param => {
                    format!("impl FnMut({params}){}", return_ty.unwrap_or_default())
                }
                _ => format!("Box<dyn FnMut({params}){}>", return_ty.unwrap_or_default()),
            };
        }
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
            "Cache" if !self.type_kinds.contains_key("Cache") => {
                "rsscript_runtime::Cache".to_string()
            }
            "Rule" if !self.type_kinds.contains_key("Rule") => "rsscript_runtime::Rule".to_string(),
            "Config" if !self.type_kinds.contains_key("Config") => {
                "rsscript_runtime::Config".to_string()
            }
            "GlobalConfig" if !self.type_kinds.contains_key("GlobalConfig") => {
                "rsscript_runtime::GlobalConfig".to_string()
            }
            "Environment" => "rsscript_runtime::Environment".to_string(),
            "FunctionObject" => "rsscript_runtime::FunctionObject".to_string(),
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
            "ImageCache" => "rsscript_runtime::ImageCache".to_string(),
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

        if self.should_wrap_in_managed_handle(ty, position) {
            format!("rsscript_runtime::Managed<{lowered}>")
        } else {
            lowered
        }
    }

    fn should_wrap_in_managed_handle(&self, ty: &TypeRef, position: ManagedPosition) -> bool {
        if !matches!(
            position,
            ManagedPosition::Param | ManagedPosition::Return | ManagedPosition::Nested
        ) {
            return false;
        }
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Class))
    }

    fn is_class_type(&self, ty: &TypeRef) -> bool {
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Class))
    }

    fn is_weak_field(&self, type_name: &str, field_name: &str) -> bool {
        self.program.items.iter().any(|item| match item {
            Item::Type(ty) if ty.name == type_name => ty
                .fields
                .iter()
                .any(|field| field.name == field_name && field.is_weak),
            _ => false,
        })
    }

    fn lower_explicit_weak_field_value(&mut self, expr: &Expr) -> String {
        if let Some(value) = explicit_weak_handle_source(expr) {
            return self.lower_runtime_weak_from_managed(value);
        }
        self.lower_expr(expr)
    }

    fn lower_runtime_weak_from_managed(&mut self, expr: &Expr) -> String {
        if let Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } = expr
        {
            let value_expr = self.lower_expr(value);
            if let Expr::Ident(name, _) = &**value
                && matches!(
                    self.param_effects.get(name),
                    Some(DataEffect::Read | DataEffect::Mut)
                )
            {
                return format!("rsscript_runtime::weak({value_expr})");
            }
            return format!("rsscript_runtime::weak(&{value_expr})");
        }
        format!("rsscript_runtime::weak(&{})", self.lower_expr(expr))
    }
}

fn explicit_weak_handle_source(expr: &Expr) -> Option<&Expr> {
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };
    if !matches!(
        callee,
        Callee::Qualified { namespace, name }
            if namespace == "Weak" && matches!(name.as_str(), "from" | "downgrade")
    ) {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.name.as_deref() == Some("value") => Some(&arg.value),
        _ => None,
    }
}

fn is_weak_upgrade_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "Weak" && name == "upgrade"
    )
}

fn lower_weak_upgrade_call(lowerer: &mut RustLowerer<'_>, args: &[CallArg]) -> String {
    let Some(arg) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("value"))
        .or_else(|| args.first())
    else {
        return "None".to_string();
    };
    let target = match &arg.value {
        Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } => lowerer.lower_expr(value),
        _ => lowerer.lower_expr(&arg.value),
    };
    format!("{target}.upgrade()")
}

fn is_result_type(ty: &TypeRef) -> bool {
    ty.name == "Result" && ty.args.len() == 2
}

fn list_element_type_ref(ty: &TypeRef) -> Option<TypeRef> {
    if ty.name == "List" && ty.args.len() == 1 {
        ty.args.first().cloned()
    } else {
        None
    }
}

fn is_result_constructor_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Call {
            callee: Callee::Name(name),
            ..
        } => matches!(name.as_str(), "Ok" | "Err"),
        _ => false,
    }
}

fn collect_function_return_types(program: &Program) -> BTreeMap<String, TypeRef> {
    let mut return_types = BTreeMap::new();
    collect_program_function_return_types(program, &mut return_types);
    for (file, source) in builtin_interfaces() {
        let interface_program = parse_source(file, source);
        collect_program_function_return_types(&interface_program, &mut return_types);
    }
    return_types
}

fn collect_function_retained_params(program: &Program) -> BTreeMap<String, BTreeSet<String>> {
    let mut retained_params = BTreeMap::new();
    for (file, source) in builtin_interfaces() {
        let interface_program = parse_source(file, source);
        collect_program_function_retained_params(&interface_program, &mut retained_params);
    }
    collect_program_function_retained_params(program, &mut retained_params);
    retained_params
}

fn collect_program_function_retained_params(
    program: &Program,
    retained_params: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let retained = function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Retains(param) => Some(param.clone()),
                EffectDecl::Name(_) => None,
            })
            .collect::<BTreeSet<_>>();
        if !retained.is_empty() {
            retained_params.insert(native_boundary_function_key(&function.name), retained);
        }
    }
}

fn collect_program_function_return_types(
    program: &Program,
    return_types: &mut BTreeMap<String, TypeRef>,
) {
    for item in &program.items {
        if let Item::Function(function) = item
            && let Some(return_ty) = &function.return_ty
        {
            return_types.insert(function.name.clone(), return_ty.clone());
        }
    }
}

fn collect_native_boundary_callees(program: &Program) -> BTreeSet<String> {
    let mut callees = BTreeSet::new();
    for (file, source) in builtin_interfaces() {
        let interface = parse_source(file, source);
        collect_native_boundary_callees_from_program(&interface, &mut callees);
    }
    collect_native_boundary_callees_from_program(program, &mut callees);
    callees
}

fn collect_native_boundary_callees_from_program(program: &Program, callees: &mut BTreeSet<String>) {
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if function.effects.iter().any(is_native_boundary) {
            callees.insert(native_boundary_function_key(&function.name));
        }
    }
}

fn native_boundary_function_key(name: &str) -> String {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        format!("{}.{}", type_root_name(namespace), name)
    } else {
        name.to_string()
    }
}

fn native_boundary_callee_key(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{}.{}", type_root_name(namespace), name),
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
        Stmt::For(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.iterable, names);
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::Match(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.value, names);
            for arm in &stmt.arms {
                collect_mutated_bindings_from_block(&arm.body, names);
            }
        }
        Stmt::Expr(expr) => collect_mutated_bindings_from_expr(expr, names),
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
        Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            collect_mutated_bindings_from_expr(value, names);
        }
        Expr::Closure { body, .. } => collect_mutated_bindings_from_block(body, names),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn closure_value_mutates_capture(expr: &Expr) -> bool {
    let Expr::Closure { body, .. } = expr else {
        return false;
    };
    let mut bound = BTreeSet::new();
    collect_closure_bound_names_from_block(body, &mut bound);
    closure_block_mutates_unbound_name(body, &bound)
}

fn collect_closure_bound_names_from_block(block: &Block, names: &mut BTreeSet<String>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                names.insert(stmt.name.clone());
            }
            Stmt::With(stmt) => {
                names.insert(stmt.binding.clone());
                collect_closure_bound_names_from_block(&stmt.body, names);
            }
            Stmt::If(stmt) => {
                collect_closure_bound_names_from_block(&stmt.then_body, names);
                if let Some(else_body) = &stmt.else_body {
                    collect_closure_bound_names_from_block(else_body, names);
                }
            }
            Stmt::Loop(stmt) => collect_closure_bound_names_from_block(&stmt.body, names),
            Stmt::For(stmt) => {
                names.insert(stmt.binding.clone());
                collect_closure_bound_names_from_block(&stmt.body, names);
            }
            Stmt::Match(stmt) => {
                for arm in &stmt.arms {
                    collect_closure_bound_names_from_block(&arm.body, names);
                }
            }
            Stmt::Return(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Expr(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

fn closure_block_mutates_unbound_name(block: &Block, bound: &BTreeSet<String>) -> bool {
    block
        .statements
        .iter()
        .any(|statement| closure_stmt_mutates_unbound_name(statement, bound))
}

fn closure_stmt_mutates_unbound_name(statement: &Stmt, bound: &BTreeSet<String>) -> bool {
    match statement {
        Stmt::Let(stmt) => stmt
            .value
            .as_ref()
            .is_some_and(|value| closure_expr_mutates_unbound_name(value, bound)),
        Stmt::Return(stmt) => stmt
            .value
            .as_ref()
            .is_some_and(|value| closure_expr_mutates_unbound_name(value, bound)),
        Stmt::With(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.resource, bound)
                || closure_block_mutates_unbound_name(&stmt.body, bound)
        }
        Stmt::If(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.condition, bound)
                || closure_block_mutates_unbound_name(&stmt.then_body, bound)
                || stmt
                    .else_body
                    .as_ref()
                    .is_some_and(|body| closure_block_mutates_unbound_name(body, bound))
        }
        Stmt::Loop(stmt) => {
            stmt.condition
                .as_ref()
                .is_some_and(|condition| closure_expr_mutates_unbound_name(condition, bound))
                || closure_block_mutates_unbound_name(&stmt.body, bound)
        }
        Stmt::For(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.iterable, bound)
                || closure_block_mutates_unbound_name(&stmt.body, bound)
        }
        Stmt::Match(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.value, bound)
                || stmt
                    .arms
                    .iter()
                    .any(|arm| closure_block_mutates_unbound_name(&arm.body, bound))
        }
        Stmt::Expr(expr) => closure_expr_mutates_unbound_name(expr, bound),
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

fn closure_expr_mutates_unbound_name(expr: &Expr, bound: &BTreeSet<String>) -> bool {
    match expr {
        Expr::Effect {
            effect: DataEffect::Mut,
            value,
            ..
        } => {
            mutable_root_ident(value).is_some_and(|name| !bound.contains(name))
                || closure_expr_mutates_unbound_name(value, bound)
        }
        Expr::Binary { left, right, .. } => {
            closure_expr_mutates_unbound_name(left, bound)
                || closure_expr_mutates_unbound_name(right, bound)
        }
        Expr::Field { base, .. } => closure_expr_mutates_unbound_name(base, bound),
        Expr::Index { base, index, .. } => {
            closure_expr_mutates_unbound_name(base, bound)
                || closure_expr_mutates_unbound_name(index, bound)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| closure_expr_mutates_unbound_name(&arg.value, bound)),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => closure_expr_mutates_unbound_name(value, bound),
        Expr::Closure { body, .. } => closure_block_mutates_unbound_name(body, bound),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => false,
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
        Stmt::For(stmt) => &stmt.span,
        Stmt::Match(stmt) => &stmt.span,
        Stmt::Break(span)
        | Stmt::Continue(span)
        | Stmt::MalformedWith(span)
        | Stmt::MalformedIf(span)
        | Stmt::MalformedLoop(span)
        | Stmt::MalformedFor(span)
        | Stmt::MalformedMatch(span)
        | Stmt::Unknown(span) => span,
        Stmt::Expr(expr) => expr.span(),
    }
}

fn lower_match_pattern(pattern: &MatchPattern) -> String {
    match pattern {
        MatchPattern::Wildcard(_) => "_".to_string(),
        MatchPattern::Variant {
            name,
            binding: Some(binding),
            ..
        } => format!("{}({})", rust_ident(name), rust_ident(binding)),
        MatchPattern::Variant { name, .. } => rust_ident(name),
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
    Bare,
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
                Some(GenericBound::Managed) => format!("{name}: rsscript_runtime::ManagedValue"),
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
    if let Some(target) = runtime_intrinsic_target(callee) {
        return target.to_string();
    }

    match callee {
        Callee::Name(name) => rust_ident(name),
        Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" => {
            format!("rsscript_runtime::ResourcePool::{}", rust_ident(name))
        }
        Callee::Qualified { namespace, name } => rust_qualified_function_ident(namespace, name),
    }
}

fn runtime_intrinsic_target(callee: &Callee) -> Option<&'static str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    runtime_abi::lookup_runtime_intrinsic(type_root_name(namespace), name)
        .map(|intrinsic| intrinsic.rust_target)
}

fn runtime_intrinsic_wants_managed_handle_arg(callee: &Callee, arg_name: Option<&str>) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    let Some(arg_name) = arg_name else {
        return false;
    };
    runtime_abi::lookup_runtime_intrinsic(type_root_name(namespace), name)
        .is_some_and(|intrinsic| intrinsic.managed_handle_args.contains(&arg_name))
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

fn is_resource_pool_try_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && name == "try_new")
}

fn is_resource_pool_borrow_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if is_resource_pool_borrow_callee(callee))
}

fn lower_required_call_arg(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    name: &str,
    index: usize,
    call_span: &Span,
) -> String {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .or_else(|| args.get(index))
        .map(|arg| lowerer.lower_expr(&arg.value))
        .unwrap_or_else(|| unreachable_lowering("validated call argument", call_span))
}

fn lower_resource_pool_new_call(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_required_call_arg(lowerer, args, "create", 0, call_span);
    let max_size = lower_required_call_arg(lowerer, args, "max_size", 1, call_span);
    format!("rsscript_runtime::ResourcePool::from_factory({max_size}, {create})")
}

fn lower_resource_pool_try_new_call(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_required_call_arg(lowerer, args, "create", 0, call_span);
    let max_size = lower_required_call_arg(lowerer, args, "max_size", 1, call_span);
    format!("rsscript_runtime::ResourcePool::try_from_factory({max_size}, {create})")
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

fn rust_function_ident(name: &str) -> String {
    name.split('.')
        .map(rust_ident)
        .collect::<Vec<_>>()
        .join("_")
}

fn rust_qualified_function_ident(namespace: &str, name: &str) -> String {
    namespace
        .split('.')
        .chain(std::iter::once(name))
        .map(rust_path_segment)
        .collect::<Vec<_>>()
        .join("_")
}

fn type_root_name(name: &str) -> &str {
    let name = name.trim().strip_prefix("fresh ").unwrap_or(name.trim());
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
            "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
            "    {}\n",
            "}}\n"
        ),
        main.name, call
    ))
}

fn lowered_feature_names(features: &[FileFeature]) -> Vec<&'static str> {
    let mut names = features
        .iter()
        .map(|feature| match feature {
            FileFeature::Local => "local",
            FileFeature::Native => "native",
            FileFeature::Unsafe => "unsafe",
            FileFeature::Async => "async",
            FileFeature::Device => "device",
            FileFeature::Ffi => "ffi",
            FileFeature::Reflection => "reflection",
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
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

fn unreachable_lowering(kind: &str, span: &Span) -> ! {
    panic!(
        "internal RSScript lowering error: unsupported {kind} reached Rust lowering at {}:{}:{}",
        span.file, span.line, span.column
    )
}

fn cargo_crate_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
