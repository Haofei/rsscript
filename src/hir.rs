use std::collections::{HashMap, HashSet};

use crate::syntax::ast::{
    DataEffect, EffectDecl, FieldDecl, FunctionDecl, Item, Param, Program as SyntaxProgram,
    TypeDecl, TypeKind,
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

#[derive(Debug, Default)]
pub struct Hir {
    signatures: HashMap<String, FunctionSig>,
    types: HashMap<String, TypeInfo>,
    fields_by_name: HashMap<String, Vec<FieldInfo>>,
}

impl Hir {
    pub fn from_syntax(program: &SyntaxProgram) -> Self {
        let mut hir = Self::default();
        hir.insert_builtins();
        for item in &program.items {
            match item {
                Item::Function(function) => hir.insert_function(function_sig_from_decl(function)),
                Item::Type(type_decl) => hir.insert_type(type_info_from_decl(type_decl)),
            }
        }
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

    fn insert_function(&mut self, signature: FunctionSig) {
        let key = match &signature.namespace {
            Some(namespace) => qualified_key(namespace, &signature.name),
            None => signature.name.clone(),
        };
        self.signatures.insert(key, signature);
    }

    fn insert_type(&mut self, type_info: TypeInfo) {
        for field in type_info.fields.values() {
            self.fields_by_name
                .entry(field.name.clone())
                .or_default()
                .push(field.clone());
        }
        self.types.insert(type_info.name.clone(), type_info);
    }

    fn insert_builtins(&mut self) {
        for signature in builtin_signatures() {
            self.insert_function(signature);
        }
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
}
