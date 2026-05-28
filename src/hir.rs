use std::collections::{HashMap, HashSet};

use crate::diagnostic::Span;
use crate::syntax::ast::{
    Block, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FunctionDecl, Item, Param,
    Program as SyntaxProgram, Stmt, TypeDecl, TypeKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamEffect {
    Read,
    Mut,
    Take,
}

impl ParamEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mut => "mut",
            Self::Take => "take",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSig {
    pub name: String,
    pub effect: Option<ParamEffect>,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub namespace: Option<String>,
    pub name: String,
    pub params: Vec<ParamSig>,
    pub return_type: Option<String>,
    pub returns_fresh: bool,
    pub retained_params: HashSet<String>,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirTypeKind {
    Class,
    Struct,
    Resource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    pub type_name: String,
    pub is_handle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub kind: HirTypeKind,
    pub fields: HashMap<String, FieldInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateSymbolKind {
    Function,
    Type,
    Constructor,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateSymbol {
    pub kind: DuplicateSymbolKind,
    pub name: String,
    pub first_span: Span,
    pub duplicate_span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedCalleeKind {
    UserFunction,
    BuiltinFunction,
    Constructor { type_kind: HirTypeKind },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallResolution {
    Resolved {
        signature: FunctionSig,
        kind: ResolvedCalleeKind,
    },
    EnumVariant,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirCallSite {
    pub function_name: String,
    pub callee: Callee,
    pub span: Span,
    pub resolution: CallResolution,
}

#[derive(Debug, Default)]
pub struct Hir {
    signatures: HashMap<String, FunctionSig>,
    types: HashMap<String, TypeInfo>,
    fields_by_name: HashMap<String, Vec<FieldInfo>>,
    duplicate_symbols: Vec<DuplicateSymbol>,
    call_sites: Vec<HirCallSite>,
    call_resolutions_by_span: HashMap<Span, CallResolution>,
}

impl Hir {
    pub fn from_syntax(program: &SyntaxProgram) -> Self {
        let mut hir = Self::default();
        hir.insert_builtins();
        let mut type_symbols = HashMap::new();
        let mut callable_symbols = HashMap::new();
        for item in &program.items {
            match item {
                Item::Function(function) => {
                    record_duplicate_symbol(
                        &mut hir.duplicate_symbols,
                        &mut callable_symbols,
                        DuplicateSymbolKind::Function,
                        &function.name,
                        &function.span,
                    );
                    hir.insert_function(function_sig_from_decl(function));
                }
                Item::Type(type_decl) => {
                    record_duplicate_fields(&mut hir.duplicate_symbols, type_decl);
                    record_duplicate_symbol(
                        &mut hir.duplicate_symbols,
                        &mut type_symbols,
                        DuplicateSymbolKind::Type,
                        &type_decl.name,
                        &type_decl.span,
                    );
                    record_duplicate_symbol(
                        &mut hir.duplicate_symbols,
                        &mut callable_symbols,
                        DuplicateSymbolKind::Constructor,
                        &type_decl.name,
                        &type_decl.span,
                    );
                    hir.insert_type(type_info_from_decl(type_decl));
                }
            }
        }
        hir.collect_call_sites(program);
        hir
    }

    pub fn resolve_function(&self, namespace: Option<&str>, name: &str) -> Option<&FunctionSig> {
        if let Some(namespace) = namespace
            && let Some(signature) = self.signatures.get(&qualified_key(namespace, name))
        {
            return Some(signature);
        }
        self.signatures.get(name)
    }

    pub fn type_info(&self, name: &str) -> Option<&TypeInfo> {
        self.types.get(name)
    }

    pub fn type_kind(&self, name: &str) -> Option<HirTypeKind> {
        self.type_info(name).map(|info| info.kind)
    }

    pub fn fields_named(&self, field_name: &str) -> impl Iterator<Item = &FieldInfo> {
        self.fields_by_name
            .get(field_name)
            .into_iter()
            .flat_map(|fields| fields.iter())
    }

    pub fn is_handle_field_name(&self, field_name: &str) -> bool {
        self.fields_named(field_name).any(|field| field.is_handle)
    }

    pub fn duplicate_symbols(&self) -> &[DuplicateSymbol] {
        &self.duplicate_symbols
    }

    pub fn call_resolution(&self, span: &Span) -> Option<&CallResolution> {
        self.call_resolutions_by_span.get(span)
    }

    pub fn resolve_call(&self, callee: &Callee) -> CallResolution {
        let call_name = callee_name(callee);
        if is_enum_variant_call(call_name) {
            return CallResolution::EnumVariant;
        }

        let signature = match callee {
            Callee::Name(name) => self.resolve_function(None, name),
            Callee::Qualified { namespace, name } => self.resolve_function(Some(namespace), name),
        };
        let Some(signature) = signature else {
            return CallResolution::Unknown;
        };
        let kind = match callee {
            Callee::Name(name) => self.type_kind(name).map_or_else(
                || function_kind(signature),
                |type_kind| ResolvedCalleeKind::Constructor { type_kind },
            ),
            Callee::Qualified { .. } => function_kind(signature),
        };

        CallResolution::Resolved {
            signature: signature.clone(),
            kind,
        }
    }

    fn insert_function(&mut self, signature: FunctionSig) {
        let key = match &signature.namespace {
            Some(namespace) => qualified_key(namespace, &signature.name),
            None => signature.name.clone(),
        };
        self.signatures.insert(key, signature);
    }

    fn insert_type(&mut self, type_info: TypeInfo) {
        let constructor = constructor_sig_from_type(&type_info);
        for field in type_info.fields.values() {
            self.fields_by_name
                .entry(field.name.clone())
                .or_default()
                .push(field.clone());
        }
        self.types.insert(type_info.name.clone(), type_info);
        self.insert_function(constructor);
    }

    fn insert_builtins(&mut self) {
        for signature in builtin_signatures() {
            self.insert_function(signature);
        }
    }

    fn collect_call_sites(&mut self, program: &SyntaxProgram) {
        let mut sites = Vec::new();
        for item in &program.items {
            let Item::Function(function) = item else {
                continue;
            };
            collect_call_sites_in_block(self, &function.name, &function.body, &mut sites);
        }

        self.call_resolutions_by_span = sites
            .iter()
            .map(|site| (site.span.clone(), site.resolution.clone()))
            .collect();
        self.call_sites = sites;
    }
}

fn collect_call_sites_in_block(
    hir: &Hir,
    function_name: &str,
    block: &Block,
    sites: &mut Vec<HirCallSite>,
) {
    for statement in &block.statements {
        collect_call_sites_in_stmt(hir, function_name, statement, sites);
    }
}

fn collect_call_sites_in_stmt(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    sites: &mut Vec<HirCallSite>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_call_sites_in_expr(hir, function_name, value, sites);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_call_sites_in_expr(hir, function_name, value, sites);
            }
        }
        Stmt::With(stmt) => {
            collect_call_sites_in_expr(hir, function_name, &stmt.resource, sites);
            collect_call_sites_in_block(hir, function_name, &stmt.body, sites);
        }
        Stmt::If(stmt) => {
            collect_call_sites_in_expr(hir, function_name, &stmt.condition, sites);
            collect_call_sites_in_block(hir, function_name, &stmt.then_body, sites);
            if let Some(else_body) = &stmt.else_body {
                collect_call_sites_in_block(hir, function_name, else_body, sites);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_call_sites_in_expr(hir, function_name, condition, sites);
            }
            collect_call_sites_in_block(hir, function_name, &stmt.body, sites);
        }
        Stmt::Expr(expr) => collect_call_sites_in_expr(hir, function_name, expr, sites),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
    }
}

fn collect_call_sites_in_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    sites: &mut Vec<HirCallSite>,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            sites.push(HirCallSite {
                function_name: function_name.to_string(),
                callee: callee.clone(),
                span: span.clone(),
                resolution: hir.resolve_call(callee),
            });
            for arg in args {
                collect_call_sites_in_expr(hir, function_name, &arg.value, sites);
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            collect_call_sites_in_expr(hir, function_name, value, sites);
        }
        Expr::Field { base, .. } => collect_call_sites_in_expr(hir, function_name, base, sites),
        Expr::Closure { body, .. } => collect_call_sites_in_block(hir, function_name, body, sites),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn function_kind(signature: &FunctionSig) -> ResolvedCalleeKind {
    if signature.is_builtin {
        ResolvedCalleeKind::BuiltinFunction
    } else {
        ResolvedCalleeKind::UserFunction
    }
}

fn is_enum_variant_call(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some" | "None" | "Result" | "Option")
}

fn callee_name(callee: &Callee) -> &str {
    match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => name,
    }
}

fn function_sig_from_decl(function: &FunctionDecl) -> FunctionSig {
    FunctionSig {
        namespace: None,
        name: function.name.clone(),
        params: function.params.iter().map(param_sig_from_decl).collect(),
        return_type: function.return_ty.as_ref().map(|ty| ty.name.clone()),
        returns_fresh: function.returns_fresh,
        retained_params: function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Retains(param) => Some(param.clone()),
                EffectDecl::Name(_) => None,
            })
            .collect(),
        is_builtin: false,
    }
}

fn record_duplicate_symbol(
    duplicates: &mut Vec<DuplicateSymbol>,
    symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
    kind: DuplicateSymbolKind,
    name: &str,
    span: &Span,
) {
    if let Some((first_kind, first_span)) = symbols.get(name) {
        duplicates.push(DuplicateSymbol {
            kind: duplicate_symbol_kind(*first_kind, kind),
            name: name.to_string(),
            first_span: first_span.clone(),
            duplicate_span: span.clone(),
        });
        return;
    }

    symbols.insert(name.to_string(), (kind, span.clone()));
}

fn record_duplicate_fields(duplicates: &mut Vec<DuplicateSymbol>, type_decl: &TypeDecl) {
    let mut fields = HashMap::new();
    for field in &type_decl.fields {
        record_duplicate_symbol(
            duplicates,
            &mut fields,
            DuplicateSymbolKind::Field,
            &format!("{}.{}", type_decl.name, field.name),
            &field.span,
        );
    }
}

fn duplicate_symbol_kind(
    first: DuplicateSymbolKind,
    duplicate: DuplicateSymbolKind,
) -> DuplicateSymbolKind {
    match (first, duplicate) {
        (DuplicateSymbolKind::Function, DuplicateSymbolKind::Function) => {
            DuplicateSymbolKind::Function
        }
        (DuplicateSymbolKind::Type, DuplicateSymbolKind::Type) => DuplicateSymbolKind::Type,
        (DuplicateSymbolKind::Field, DuplicateSymbolKind::Field) => DuplicateSymbolKind::Field,
        _ => DuplicateSymbolKind::Constructor,
    }
}

fn param_sig_from_decl(param: &Param) -> ParamSig {
    ParamSig {
        name: param.name.clone(),
        effect: param.effect.map(param_effect_from_data_effect),
        type_name: param.ty.name.clone(),
    }
}

fn param_effect_from_data_effect(effect: DataEffect) -> ParamEffect {
    match effect {
        DataEffect::Read => ParamEffect::Read,
        DataEffect::Mut => ParamEffect::Mut,
        DataEffect::Take => ParamEffect::Take,
    }
}

fn type_info_from_decl(type_decl: &TypeDecl) -> TypeInfo {
    TypeInfo {
        name: type_decl.name.clone(),
        kind: type_kind_from_decl(type_decl.kind),
        fields: type_decl
            .fields
            .iter()
            .map(|field| (field.name.clone(), field_info_from_decl(field)))
            .collect(),
    }
}

fn type_kind_from_decl(kind: TypeKind) -> HirTypeKind {
    match kind {
        TypeKind::Class => HirTypeKind::Class,
        TypeKind::Struct => HirTypeKind::Struct,
        TypeKind::Resource => HirTypeKind::Resource,
    }
}

fn field_info_from_decl(field: &FieldDecl) -> FieldInfo {
    FieldInfo {
        name: field.name.clone(),
        type_name: field.ty.name.clone(),
        is_handle: field.is_handle,
    }
}

fn constructor_sig_from_type(type_info: &TypeInfo) -> FunctionSig {
    let mut fields: Vec<&FieldInfo> = type_info.fields.values().collect();
    fields.sort_by(|left, right| left.name.cmp(&right.name));

    FunctionSig {
        namespace: None,
        name: type_info.name.clone(),
        params: fields
            .into_iter()
            .map(|field| ParamSig {
                name: field.name.clone(),
                effect: None,
                type_name: field.type_name.clone(),
            })
            .collect(),
        return_type: Some(type_info.name.clone()),
        returns_fresh: true,
        retained_params: HashSet::new(),
        is_builtin: false,
    }
}

fn qualified_key(namespace: &str, name: &str) -> String {
    format!("{namespace}.{name}")
}

fn builtin_signatures() -> Vec<FunctionSig> {
    vec![
        builtin(
            "Image",
            "load",
            &[param("path", ParamEffect::Read, "Path")],
            Some("Image"),
            true,
            &[],
        ),
        builtin(
            "Image",
            "resize",
            &[
                param("image", ParamEffect::Mut, "Image"),
                copy_param("width", "Int"),
                copy_param("height", "Int"),
            ],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Image",
            "normalize",
            &[param("image", ParamEffect::Mut, "Image")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Image",
            "sharpen",
            &[param("image", ParamEffect::Mut, "Image")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Image",
            "save",
            &[
                param("image", ParamEffect::Read, "Image"),
                param("path", ParamEffect::Read, "Path"),
            ],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Image",
            "inspect",
            &[param("image", ParamEffect::Read, "Image")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "ImageCache",
            "store",
            &[
                param("cache", ParamEffect::Mut, "ImageCache"),
                param("image", ParamEffect::Read, "Image"),
            ],
            Some("Unit"),
            false,
            &["image"],
        ),
        builtin(
            "File",
            "open",
            &[param("path", ParamEffect::Read, "Path")],
            Some("File"),
            false,
            &[],
        ),
        builtin(
            "File",
            "open_read",
            &[param("path", ParamEffect::Read, "Path")],
            Some("File"),
            false,
            &[],
        ),
        builtin(
            "File",
            "open_write",
            &[param("path", ParamEffect::Read, "Path")],
            Some("File"),
            false,
            &[],
        ),
        builtin(
            "File",
            "read_all",
            &[param("file", ParamEffect::Mut, "File")],
            Some("Bytes"),
            true,
            &[],
        ),
        builtin(
            "File",
            "write",
            &[
                param("file", ParamEffect::Mut, "File"),
                param("data", ParamEffect::Read, "Bytes"),
            ],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "OS",
            "close",
            &[copy_param("fd", "Fd")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Map",
            "insert",
            &[
                param("map", ParamEffect::Mut, "Map"),
                param("key", ParamEffect::Read, "K"),
                param("value", ParamEffect::Read, "V"),
            ],
            Some("Unit"),
            false,
            &["value"],
        ),
        builtin(
            "ResourcePool",
            "new",
            &[
                copy_param("create", "Closure"),
                copy_param("max_size", "Int"),
            ],
            Some("ResourcePool"),
            true,
            &[],
        ),
        builtin(
            "ResourcePool",
            "borrow",
            &[param("pool", ParamEffect::Mut, "ResourcePool")],
            None,
            false,
            &[],
        ),
        builtin(
            "RowBuffer",
            "new",
            &[copy_param("size", "Int")],
            Some("RowBuffer"),
            true,
            &[],
        ),
        builtin(
            "Json",
            "parse",
            &[param("text", ParamEffect::Read, "String")],
            Some("JsonValue"),
            true,
            &[],
        ),
        builtin(
            "Json",
            "field_string",
            &[
                param("value", ParamEffect::Read, "JsonValue"),
                param("name", ParamEffect::Read, "String"),
            ],
            Some("String"),
            true,
            &[],
        ),
        builtin(
            "Csv",
            "read_into",
            &[
                param("file", ParamEffect::Mut, "File"),
                param("buffer", ParamEffect::Mut, "RowBuffer"),
            ],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Csv",
            "parse_row",
            &[param("buffer", ParamEffect::Read, "RowBuffer")],
            Some("Row"),
            true,
            &[],
        ),
        builtin(
            "Int",
            "add",
            &[copy_param("left", "Int"), copy_param("right", "Int")],
            Some("Int"),
            false,
            &[],
        ),
        builtin(
            "List",
            "consume",
            &[param("list", ParamEffect::Take, "List")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Buffer",
            "consume",
            &[param("buffer", ParamEffect::Take, "Buffer")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "String",
            "concat",
            &[
                param("left", ParamEffect::Read, "String"),
                param("right", ParamEffect::Read, "String"),
            ],
            Some("String"),
            true,
            &[],
        ),
        builtin(
            "Log",
            "write",
            &[copy_param("message", "String")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "DbConnection",
            "open",
            &[param("url", ParamEffect::Read, "Url")],
            Some("DbConnection"),
            true,
            &[],
        ),
        builtin(
            "DbConnection",
            "query",
            &[
                param("conn", ParamEffect::Mut, "DbConnection"),
                param("sql", ParamEffect::Read, "String"),
            ],
            None,
            false,
            &[],
        ),
        builtin(
            "Db",
            "close",
            &[copy_param("fd", "Fd")],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "RuleLoader",
            "load_rules",
            &[param("path", ParamEffect::Read, "Path")],
            Some("List"),
            true,
            &[],
        ),
        builtin(
            "GlobalConfig",
            "replace",
            &[
                param("global", ParamEffect::Mut, "GlobalConfig"),
                param("value", ParamEffect::Read, "Config"),
            ],
            Some("Unit"),
            false,
            &["value"],
        ),
        builtin(
            "Cache",
            "lookup",
            &[
                param("cache", ParamEffect::Read, "Cache"),
                param("key", ParamEffect::Read, "String"),
            ],
            None,
            false,
            &[],
        ),
        builtin(
            "Cache",
            "get",
            &[param("cache", ParamEffect::Read, "Cache")],
            None,
            false,
            &[],
        ),
        builtin(
            "Request",
            "path",
            &[param("request", ParamEffect::Read, "Request")],
            Some("String"),
            false,
            &[],
        ),
        builtin(
            "Counter",
            "add",
            &[
                param("counter", ParamEffect::Mut, "Counter"),
                copy_param("amount", "Int"),
            ],
            Some("Unit"),
            false,
            &[],
        ),
        builtin(
            "Counter",
            "value",
            &[param("counter", ParamEffect::Read, "Counter")],
            Some("Int"),
            false,
            &[],
        ),
        builtin(
            "FunctionObject",
            "new",
            &[param("closure", ParamEffect::Read, "Environment")],
            Some("FunctionObject"),
            true,
            &[],
        ),
    ]
}

fn builtin(
    namespace: &str,
    name: &str,
    params: &[ParamSig],
    return_type: Option<&str>,
    returns_fresh: bool,
    retained_params: &[&str],
) -> FunctionSig {
    FunctionSig {
        namespace: Some(namespace.to_string()),
        name: name.to_string(),
        params: params.to_vec(),
        return_type: return_type.map(str::to_string),
        returns_fresh,
        retained_params: retained_params
            .iter()
            .map(|param| (*param).to_string())
            .collect(),
        is_builtin: true,
    }
}

fn param(name: &str, effect: ParamEffect, type_name: &str) -> ParamSig {
    ParamSig {
        name: name.to_string(),
        effect: Some(effect),
        type_name: type_name.to_string(),
    }
}

fn copy_param(name: &str, type_name: &str) -> ParamSig {
    ParamSig {
        name: name.to_string(),
        effect: None,
        type_name: type_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_source;

    #[test]
    fn collects_type_kinds_and_handle_fields() {
        let source = r#"
mode: uses-local

class User {
    name: String
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Session {
    user: handle User
    file_name: String
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);

        assert_eq!(hir.type_kind("User"), Some(HirTypeKind::Class));
        assert_eq!(hir.type_kind("File"), Some(HirTypeKind::Resource));
        assert_eq!(hir.type_kind("Session"), Some(HirTypeKind::Struct));

        let user_field = hir.fields_named("user").next().expect("user field exists");
        assert_eq!(user_field.type_name, "User");
        assert!(user_field.is_handle);
        let session = hir.type_info("Session").expect("session type exists");
        assert!(session.fields["user"].is_handle);
        assert!(!session.fields["file_name"].is_handle);
        assert!(hir.is_handle_field_name("user"));
        assert!(!hir.is_handle_field_name("file_name"));
    }

    #[test]
    fn keeps_builtin_and_user_function_signatures() {
        let source = r#"
mode: managed

fn cache_put(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))
{
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);

        assert!(hir.resolve_function(Some("Image"), "resize").is_some());

        let signature = hir
            .resolve_function(None, "cache_put")
            .expect("user signature exists");
        assert!(signature.retained_params.contains("value"));
        assert_eq!(signature.params[0].effect, Some(ParamEffect::Mut));
        assert_eq!(signature.params[1].effect, Some(ParamEffect::Read));
        assert_eq!(signature.return_type.as_deref(), Some("Unit"));

        let load = hir
            .resolve_function(Some("Image"), "load")
            .expect("builtin signature exists");
        assert_eq!(load.return_type.as_deref(), Some("Image"));
    }

    #[test]
    fn records_duplicate_callable_symbols() {
        let source = r#"
mode: managed

struct Image {
    pixels: Buffer
}

fn Image(path: read Path) -> Image {
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let duplicate = hir
            .duplicate_symbols()
            .first()
            .expect("constructor/function duplicate is recorded");

        assert_eq!(duplicate.kind, DuplicateSymbolKind::Constructor);
        assert_eq!(duplicate.name, "Image");
        assert_eq!(duplicate.first_span.line, 4);
        assert_eq!(duplicate.duplicate_span.line, 8);
    }

    #[test]
    fn records_duplicate_fields() {
        let source = r#"
mode: managed

struct Response {
    status: Int
    status: String
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let duplicate = hir
            .duplicate_symbols()
            .first()
            .expect("duplicate field is recorded");

        assert_eq!(duplicate.kind, DuplicateSymbolKind::Field);
        assert_eq!(duplicate.name, "Response.status");
        assert_eq!(duplicate.first_span.line, 5);
        assert_eq!(duplicate.duplicate_span.line, 6);
    }

    #[test]
    fn resolves_body_call_sites() {
        let source = r#"
mode: managed

struct Response {
    status: Int
    body: String
}

fn render(body: read String) -> Result<fresh Response, HttpError> {
    Log.write(message: read body)
    Missing.call(value: read body)
    return Response(status: 200, body: read body)
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let sites = &hir.call_sites;

        assert_eq!(sites.len(), 3);
        assert!(matches!(
            sites[0].resolution,
            CallResolution::Resolved {
                kind: ResolvedCalleeKind::BuiltinFunction,
                ..
            }
        ));
        assert!(matches!(sites[1].resolution, CallResolution::Unknown));
        assert!(matches!(
            sites[2].resolution,
            CallResolution::Resolved {
                kind: ResolvedCalleeKind::Constructor {
                    type_kind: HirTypeKind::Struct
                },
                ..
            }
        ));
    }
}
