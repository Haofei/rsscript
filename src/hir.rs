use std::collections::{HashMap, HashSet};

use crate::syntax::ast::{
    DataEffect, EffectDecl, FunctionDecl, Item, Param, Program as SyntaxProgram,
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
    pub returns_fresh: bool,
    pub retained_params: HashSet<String>,
    pub is_builtin: bool,
}

#[derive(Debug, Default)]
pub struct Hir {
    signatures: HashMap<String, FunctionSig>,
}

impl Hir {
    pub fn from_syntax(program: &SyntaxProgram) -> Self {
        let mut hir = Self::default();
        hir.insert_builtins();
        for item in &program.items {
            if let Item::Function(function) = item {
                hir.insert(function_sig_from_decl(function));
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

    fn insert(&mut self, signature: FunctionSig) {
        let key = match &signature.namespace {
            Some(namespace) => qualified_key(namespace, &signature.name),
            None => signature.name.clone(),
        };
        self.signatures.insert(key, signature);
    }

    fn insert_builtins(&mut self) {
        for signature in builtin_signatures() {
            self.insert(signature);
        }
    }
}

fn function_sig_from_decl(function: &FunctionDecl) -> FunctionSig {
    FunctionSig {
        namespace: None,
        name: function.name.clone(),
        params: function.params.iter().map(param_sig_from_decl).collect(),
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

fn qualified_key(namespace: &str, name: &str) -> String {
    format!("{namespace}.{name}")
}

fn builtin_signatures() -> Vec<FunctionSig> {
    vec![
        builtin(
            "Image",
            "load",
            &[param("path", ParamEffect::Read, "Path")],
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
            false,
            &[],
        ),
        builtin(
            "Image",
            "normalize",
            &[param("image", ParamEffect::Mut, "Image")],
            false,
            &[],
        ),
        builtin(
            "Image",
            "sharpen",
            &[param("image", ParamEffect::Mut, "Image")],
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
            false,
            &[],
        ),
        builtin(
            "Image",
            "inspect",
            &[param("image", ParamEffect::Read, "Image")],
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
            false,
            &["image"],
        ),
        builtin(
            "File",
            "open",
            &[param("path", ParamEffect::Read, "Path")],
            false,
            &[],
        ),
        builtin(
            "File",
            "open_read",
            &[param("path", ParamEffect::Read, "Path")],
            false,
            &[],
        ),
        builtin(
            "File",
            "open_write",
            &[param("path", ParamEffect::Read, "Path")],
            false,
            &[],
        ),
        builtin(
            "File",
            "read_all",
            &[param("file", ParamEffect::Mut, "File")],
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
            false,
            &["value"],
        ),
        builtin(
            "ResourcePool",
            "borrow",
            &[param("pool", ParamEffect::Mut, "ResourcePool")],
            false,
            &[],
        ),
        builtin(
            "Json",
            "parse",
            &[param("text", ParamEffect::Read, "String")],
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
            false,
            &[],
        ),
        builtin(
            "Csv",
            "parse_row",
            &[param("buffer", ParamEffect::Read, "RowBuffer")],
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
            false,
            &[],
        ),
        builtin(
            "RuleLoader",
            "load_rules",
            &[param("path", ParamEffect::Read, "Path")],
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
            false,
            &[],
        ),
        builtin(
            "Cache",
            "get",
            &[param("cache", ParamEffect::Read, "Cache")],
            false,
            &[],
        ),
        builtin(
            "Request",
            "path",
            &[param("request", ParamEffect::Read, "Request")],
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
            false,
            &[],
        ),
        builtin(
            "Counter",
            "value",
            &[param("counter", ParamEffect::Read, "Counter")],
            false,
            &[],
        ),
    ]
}

fn builtin(
    namespace: &str,
    name: &str,
    params: &[ParamSig],
    returns_fresh: bool,
    retained_params: &[&str],
) -> FunctionSig {
    FunctionSig {
        namespace: Some(namespace.to_string()),
        name: name.to_string(),
        params: params.to_vec(),
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
