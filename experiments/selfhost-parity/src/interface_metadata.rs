use crate::syntax::ast::{DataEffect, Item, TypeRef};
use crate::syntax::parse_source;
use rsscript_text::{type_arg_names, type_root_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterfaceMetadata {
    pub(crate) functions: Vec<InterfaceFunctionMetadata>,
    pub(crate) types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterfaceFunctionMetadata {
    pub(crate) namespace: Option<String>,
    pub(crate) method: String,
    pub(crate) return_type: Option<String>,
    pub(crate) params: Vec<InterfaceParamMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterfaceParamMetadata {
    pub(crate) name: String,
    pub(crate) effect: Option<DataEffect>,
    pub(crate) type_name: String,
}

pub(crate) fn collect_interface_metadata(interfaces: &[(&str, &str)]) -> InterfaceMetadata {
    let mut functions = Vec::new();
    let mut types = Vec::new();
    for (path, source) in interfaces {
        let program = parse_source(path, source);
        for item in program.items {
            match item {
                Item::Function(function) => {
                    let (namespace, method) = split_function_name(&function.name);
                    functions.push(InterfaceFunctionMetadata {
                        namespace,
                        method,
                        return_type: function.return_ty.as_ref().map(type_ref_name),
                        params: function
                            .params
                            .iter()
                            .map(|param| InterfaceParamMetadata {
                                name: param.name.clone(),
                                effect: param.effective_effect(),
                                type_name: type_ref_name(&param.ty),
                            })
                            .collect(),
                    });
                }
                Item::Type(ty) => types.push(ty.name),
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }
    types.sort();
    types.dedup();
    InterfaceMetadata { functions, types }
}

pub(crate) fn format_selfhost_interface_metadata_rss(metadata: &InterfaceMetadata) -> String {
    let mut out = String::from(
        "module selfhost.interfaces\n\n\
         fn is_builtin_type(name: read String) -> Bool {\n",
    );
    for name in rsscript_semantics::BUILTIN_TYPE_NAMES {
        out.push_str(&format!(
            "    if name == {} {{ return true }}\n",
            rss_string(name)
        ));
    }
    out.push_str("    return false\n}\n\n");

    out.push_str("fn is_stdlib_type(name: read String) -> Bool {\n");
    for name in &metadata.types {
        out.push_str(&format!(
            "    if name == {} {{ return true }}\n",
            rss_string(name)
        ));
    }
    out.push_str("    return false\n}\n\n");

    out.push_str(
        "fn generated_stdlib_return_type(ns: read String, method: read String) -> String {\n",
    );
    for function in metadata
        .functions
        .iter()
        .filter(|function| function.namespace.is_some())
    {
        if let Some(return_type) = &function.return_type {
            out.push_str(&format!(
                "    if ns == {} && method == {} {{ return {} }}\n",
                rss_string(function.namespace.as_deref().unwrap()),
                rss_string(&function.method),
                rss_string(return_type),
            ));
        }
    }
    out.push_str("    return \"\"\n}\n\n");

    out.push_str(
        "fn generated_stdlib_error_type(ns: read String, method: read String) -> String {\n",
    );
    for function in metadata
        .functions
        .iter()
        .filter(|function| function.namespace.is_some())
    {
        if let Some(error_type) = function.return_type.as_deref().and_then(result_error_type) {
            out.push_str(&format!(
                "    if ns == {} && method == {} {{ return {} }}\n",
                rss_string(function.namespace.as_deref().unwrap()),
                rss_string(&function.method),
                rss_string(error_type),
            ));
        }
    }
    out.push_str("    return \"\"\n}\n\n");

    out.push_str(
        "fn generated_stdlib_param_type(ns: read String, method: read String, pname: read String) -> String {\n",
    );
    for function in metadata
        .functions
        .iter()
        .filter(|function| function.namespace.is_some())
    {
        for param in &function.params {
            out.push_str(&format!(
                "    if ns == {} && method == {} && pname == {} {{ return {} }}\n",
                rss_string(function.namespace.as_deref().unwrap()),
                rss_string(&function.method),
                rss_string(&param.name),
                rss_string(&param.type_name),
            ));
        }
    }
    out.push_str("    return \"\"\n}\n\n");

    out.push_str(
        "fn generated_stdlib_param_name_at(ns: read String, method: read String, index: read Int) -> String {\n",
    );
    for function in metadata
        .functions
        .iter()
        .filter(|function| function.namespace.is_some())
    {
        for (index, param) in function.params.iter().enumerate() {
            out.push_str(&format!(
                "    if ns == {} && method == {} && index == {} {{ return {} }}\n",
                rss_string(function.namespace.as_deref().unwrap()),
                rss_string(&function.method),
                index,
                rss_string(&param.name),
            ));
        }
    }
    out.push_str("    return \"\"\n}\n\n");

    out.push_str(
        "fn generated_stdlib_param_effect(ns: read String, method: read String, pname: read String) -> String {\n",
    );
    for function in metadata
        .functions
        .iter()
        .filter(|function| function.namespace.is_some())
    {
        for param in &function.params {
            if let Some(effect) = param.effect {
                out.push_str(&format!(
                    "    if ns == {} && method == {} && pname == {} {{ return {} }}\n",
                    rss_string(function.namespace.as_deref().unwrap()),
                    rss_string(&function.method),
                    rss_string(&param.name),
                    rss_string(effect.as_str()),
                ));
            }
        }
    }
    out.push_str("    return \"\"\n}\n");
    out
}

fn split_function_name(name: &str) -> (Option<String>, String) {
    if let Some((namespace, method)) = name.rsplit_once('.') {
        (Some(namespace.to_string()), method.to_string())
    } else {
        (None, name.to_string())
    }
}

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = match ty.effective_fn_param_effect(index) {
                    Some(effect) => format!("{} ", effect.as_str()),
                    None => String::new(),
                };
                format!("{prefix}{}", type_ref_name(param))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

fn result_error_type(type_name: &str) -> Option<&str> {
    if type_root_name(type_name) != "Result" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.get(1).copied())
}

fn rss_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::default_interfaces;

    fn metadata() -> InterfaceMetadata {
        let interfaces = default_interfaces().collect::<Vec<_>>();
        collect_interface_metadata(&interfaces)
    }

    fn find_function<'a>(
        metadata: &'a InterfaceMetadata,
        namespace: &str,
        method: &str,
    ) -> &'a InterfaceFunctionMetadata {
        metadata
            .functions
            .iter()
            .find(|function| {
                function.namespace.as_deref() == Some(namespace) && function.method == method
            })
            .unwrap_or_else(|| panic!("missing generated metadata for {namespace}.{method}"))
    }

    fn find_param<'a>(
        function: &'a InterfaceFunctionMetadata,
        name: &str,
    ) -> &'a InterfaceParamMetadata {
        function
            .params
            .iter()
            .find(|param| param.name == name)
            .unwrap_or_else(|| panic!("missing generated metadata for parameter {name}"))
    }

    #[test]
    fn selfhost_interface_metadata_rss_contains_stdlib_signatures() {
        let metadata = metadata();
        let rss = format_selfhost_interface_metadata_rss(&metadata);
        assert!(rss.contains("module selfhost.interfaces"));
        assert!(rss.contains("if name == \"Int\" { return true }"));
        assert!(!rss.contains("if name == \"ProcessRequest\" { return true }"));
        assert!(rss.contains("ns == \"Json\" && method == \"parse\""));
        assert!(rss.contains("return \"Result<fresh JsonValue, JsonError>\""));
        assert!(!rss.contains("ns == \"File\" && method == \"write\""));
    }

    #[test]
    fn selfhost_known_types_come_from_canonical_sources() {
        let metadata = metadata();
        assert!(!metadata.types.iter().any(|name| name == "ProcessRequest"));
        assert!(metadata.types.iter().any(|name| name == "JsonValue"));
        assert!(rsscript_semantics::BUILTIN_TYPE_NAMES.contains(&"Int"));
        assert!(rsscript_semantics::BUILTIN_TYPE_NAMES.contains(&"Result"));
    }

    #[test]
    fn selfhost_interface_metadata_keeps_param_types_from_rssi() {
        let metadata = metadata();

        let concat = find_function(&metadata, "String", "concat");
        assert_eq!(find_param(concat, "left").type_name, "String");
        assert_eq!(find_param(concat, "left").effect, Some(DataEffect::Read));
        assert_eq!(find_param(concat, "right").type_name, "String");
    }

    #[test]
    fn selfhost_interface_metadata_keeps_return_and_error_types_from_rssi() {
        let metadata = metadata();

        let parse = find_function(&metadata, "Json", "parse");
        assert_eq!(
            parse.return_type.as_deref(),
            Some("Result<fresh JsonValue, JsonError>")
        );

        let rss = format_selfhost_interface_metadata_rss(&metadata);
        assert!(rss.contains("if ns == \"Json\" && method == \"parse\" { return \"JsonError\" }"));
        assert!(rss.contains(
            "if ns == \"String\" && method == \"concat\" && pname == \"left\" { return \"String\" }"
        ));
        assert!(rss.contains(
            "if ns == \"String\" && method == \"concat\" && index == 0 { return \"left\" }"
        ));
        assert!(!rss.contains("if ns == \"File\" && method == \"write\""));
    }
}
