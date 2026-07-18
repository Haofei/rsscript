use crate::syntax::ast::{DataEffect, Item, TypeRef};
use crate::syntax::parse_source;
use crate::text_util::{type_arg_names, type_root_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterfaceMetadata {
    pub(crate) functions: Vec<InterfaceFunctionMetadata>,
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
    for (path, source) in interfaces {
        let program = parse_source(path, source);
        for item in program.items {
            if let Item::Function(function) = item {
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
        }
    }
    InterfaceMetadata { functions }
}

pub(crate) fn format_selfhost_interface_metadata_rss(metadata: &InterfaceMetadata) -> String {
    let mut out = String::from(
        "module selfhost.interfaces\n\n\
         fn generated_stdlib_return_type(ns: read String, method: read String) -> String {\n",
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
        assert!(rss.contains("ns == \"Image\" && method == \"load\""));
        assert!(rss.contains("return \"Result<fresh Image, ImageError>\""));
        assert!(rss.contains("ns == \"File\" && method == \"write\" && pname == \"file\""));
        assert!(rss.contains("return \"mut\""));
    }

    #[test]
    fn selfhost_interface_metadata_keeps_param_types_from_rssi() {
        let metadata = metadata();

        let concat = find_function(&metadata, "String", "concat");
        assert_eq!(find_param(concat, "left").type_name, "String");
        assert_eq!(find_param(concat, "left").effect, Some(DataEffect::Read));
        assert_eq!(find_param(concat, "right").type_name, "String");

        let resize = find_function(&metadata, "Image", "resize");
        assert_eq!(find_param(resize, "image").type_name, "Image");
        assert_eq!(find_param(resize, "image").effect, Some(DataEffect::Mut));
        assert_eq!(find_param(resize, "width").type_name, "Int");
        assert_eq!(find_param(resize, "height").type_name, "Int");
    }

    #[test]
    fn selfhost_interface_metadata_keeps_return_and_error_types_from_rssi() {
        let metadata = metadata();

        let load = find_function(&metadata, "Image", "load");
        assert_eq!(
            load.return_type.as_deref(),
            Some("Result<fresh Image, ImageError>")
        );

        let rss = format_selfhost_interface_metadata_rss(&metadata);
        assert!(rss.contains("if ns == \"Image\" && method == \"load\" { return \"ImageError\" }"));
        assert!(rss.contains(
            "if ns == \"String\" && method == \"concat\" && pname == \"left\" { return \"String\" }"
        ));
        assert!(rss.contains(
            "if ns == \"String\" && method == \"concat\" && index == 0 { return \"left\" }"
        ));
        assert!(rss.contains(
            "if ns == \"Image\" && method == \"resize\" && pname == \"image\" { return \"mut\" }"
        ));
    }
}
