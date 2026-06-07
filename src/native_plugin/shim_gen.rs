//! Generates a per-package cdylib "shim" crate that wraps a native Rust crate's
//! typed functions as [`NativeInterpreterFn`]s and exposes them through the
//! `rss-native-abi` registry. The host (the VM) `dlopen`s the built cdylib and
//! pulls the bindings out, so native packages run in the VM with no hand-written
//! per-package glue.

/// A native value/argument shape the shim knows how to convert between
/// `NativeValue` and the native crate's concrete Rust types. Composable, so
/// nested shapes like `List<Int>` or `Option<String>` are supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShimType {
    Unit,
    String,
    Int,
    Float,
    Bool,
    Bytes,
    /// `Path` <-> `std::path::PathBuf` (bridged as a string).
    Path,
    List(Box<ShimType>),
    Option(Box<ShimType>),
}

/// How a binding's return value is shaped at the RSS level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShimReturn {
    /// `Result<T, String>`: errors come back as the native crate's `Err(String)`.
    Result(ShimType),
    /// A bare value with no `Result` wrapper.
    Plain(ShimType),
}

/// One native binding to wrap: its RSS symbol, the Rust path to call, parameter
/// shapes (in order), the return shape, and which parameter positions are `mut`.
///
/// For a binding with `mut` params the shim passes them by `&mut`, then returns
/// an envelope `List[result, mutated...]` (mutated values in `mut_indices` order)
/// so the host can write the in-place mutations back to the caller's registers.
#[derive(Debug, Clone)]
pub(crate) struct ShimBinding {
    pub symbol: String,
    pub rust_path: String,
    pub params: Vec<ShimType>,
    pub ret: ShimReturn,
    pub mut_indices: Vec<usize>,
}

/// The generated crate's two files.
pub(crate) struct ShimCrate {
    pub cargo_toml: String,
    pub lib_rs: String,
}

impl ShimType {
    /// Owned Rust type a `NativeValue` extracts into for this shape.
    fn owned_rust_ty(&self) -> String {
        match self {
            ShimType::Unit => "()".to_string(),
            ShimType::String => "String".to_string(),
            ShimType::Int => "i64".to_string(),
            ShimType::Float => "f64".to_string(),
            ShimType::Bool => "bool".to_string(),
            ShimType::Bytes => "Vec<u8>".to_string(),
            ShimType::Path => "std::path::PathBuf".to_string(),
            ShimType::List(inner) => format!("Vec<{}>", inner.owned_rust_ty()),
            ShimType::Option(inner) => format!("Option<{}>", inner.owned_rust_ty()),
        }
    }

    /// A Rust expression of type `Result<Owned, String>` converting the
    /// `NativeValue` expression `src` into the owned Rust type.
    fn from_native_expr(&self, src: &str) -> String {
        match self {
            ShimType::Unit => {
                format!("match {src} {{ NativeValue::Unit => Ok(()), other => Err(format!(\"expected Unit, got {{other:?}}\")) }}")
            }
            ShimType::String => {
                format!("match {src} {{ NativeValue::String(v) => Ok(v), other => Err(format!(\"expected String, got {{other:?}}\")) }}")
            }
            ShimType::Int => {
                format!("match {src} {{ NativeValue::Int(v) => Ok(v), other => Err(format!(\"expected Int, got {{other:?}}\")) }}")
            }
            ShimType::Float => {
                format!("match {src} {{ NativeValue::Float(v) => Ok(v), other => Err(format!(\"expected Float, got {{other:?}}\")) }}")
            }
            ShimType::Bool => {
                format!("match {src} {{ NativeValue::Bool(v) => Ok(v), other => Err(format!(\"expected Bool, got {{other:?}}\")) }}")
            }
            ShimType::Bytes => {
                format!("match {src} {{ NativeValue::Bytes(v) => Ok(v), other => Err(format!(\"expected Bytes, got {{other:?}}\")) }}")
            }
            ShimType::Path => {
                format!("match {src} {{ NativeValue::String(v) => Ok(std::path::PathBuf::from(v)), other => Err(format!(\"expected Path, got {{other:?}}\")) }}")
            }
            ShimType::List(inner) => {
                let inner_conv = inner.from_native_expr("__item");
                format!("match {src} {{ NativeValue::List(__items) => __items.into_iter().map(|__item| {inner_conv}).collect::<Result<Vec<_>, String>>(), other => Err(format!(\"expected List, got {{other:?}}\")) }}")
            }
            ShimType::Option(inner) => {
                let inner_conv = inner.from_native_expr("__inner");
                format!("match {src} {{ NativeValue::Variant {{ name, mut fields }} if name == \"Some\" => match fields.remove(\"value\") {{ Some(__inner) => ({inner_conv}).map(Some), None => Err(\"Some payload missing value\".to_string()) }}, NativeValue::Variant {{ name, .. }} if name == \"None\" => Ok(None), other => Err(format!(\"expected Option, got {{other:?}}\")) }}")
            }
        }
    }

    /// Expression passing an owned value named `name` to the native call (by
    /// reference for borrowing types).
    fn call_arg(&self, name: &str) -> String {
        match self {
            ShimType::String
            | ShimType::Bytes
            | ShimType::Path
            | ShimType::List(_) => format!("&{name}"),
            _ => name.to_string(),
        }
    }

    /// A Rust expression of type `NativeValue` converting the owned value `expr`.
    fn to_native_expr(&self, expr: &str) -> String {
        match self {
            ShimType::Unit => format!("{{ let _ = {expr}; NativeValue::Unit }}"),
            ShimType::String => format!("NativeValue::String({expr})"),
            ShimType::Int => format!("NativeValue::Int({expr})"),
            ShimType::Float => format!("NativeValue::Float({expr})"),
            ShimType::Bool => format!("NativeValue::Bool({expr})"),
            ShimType::Bytes => format!("NativeValue::Bytes({expr})"),
            ShimType::Path => {
                format!("NativeValue::String({expr}.to_string_lossy().into_owned())")
            }
            ShimType::List(inner) => {
                let inner_conv = inner.to_native_expr("__elem");
                format!("NativeValue::List({expr}.into_iter().map(|__elem| {inner_conv}).collect())")
            }
            ShimType::Option(inner) => {
                let inner_conv = inner.to_native_expr("__payload");
                format!("match {expr} {{ Some(__payload) => __some({inner_conv}), None => __none() }}")
            }
        }
    }
}

/// Generate the cdylib shim crate for `bindings`. `native_deps` are the
/// `(crate_name, absolute_path)` pairs of the native crates the bindings call
/// into; `native_abi_path` is the path to the shared `rss-native-abi` crate.
pub(crate) fn generate_shim_crate(
    shim_crate_name: &str,
    native_deps: &[(String, String)],
    native_abi_path: &str,
    bindings: &[ShimBinding],
) -> ShimCrate {
    let mut native_dep_toml = String::new();
    for (crate_name, path) in native_deps {
        native_dep_toml.push_str(&format!("{crate_name} = {{ path = {path:?} }}\n"));
    }
    // An empty `[workspace]` makes the generated crate its own workspace root so
    // cargo does not try to attach it to the repo workspace via path deps.
    let cargo_toml = format!(
        "[package]\nname = \"{shim_crate_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[lib]\nname = \"{shim_crate_name}\"\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nrss-native-abi = {{ path = {native_abi_path:?} }}\n{native_dep_toml}",
    );

    let mut functions = String::new();
    let mut entries = String::new();
    for (index, binding) in bindings.iter().enumerate() {
        let fn_name = format!("shim_{index}");
        functions.push_str(&render_shim_function(&fn_name, binding));
        functions.push('\n');
        let symbol = &binding.symbol;
        entries.push_str(&format!(
            "        NativeBindingEntry {{ name: c\"{symbol}\".as_ptr(), func: {fn_name} }},\n"
        ));
    }

    let lib_rs = format!(
        "// @generated by rsscript native_plugin::shim_gen -- do not edit.\n\
#![allow(clippy::all)]\n\
use std::collections::BTreeMap;\n\n\
use rss_native_abi::{{NativeBindingEntry, NativeRegistry, NativeValue}};\n\n\
fn __ok(value: NativeValue) -> NativeValue {{\n    let mut fields = BTreeMap::new();\n    fields.insert(\"value\".to_string(), value);\n    NativeValue::Variant {{ name: \"Ok\".to_string(), fields }}\n}}\n\n\
fn __err(message: String) -> NativeValue {{\n    let mut fields = BTreeMap::new();\n    fields.insert(\"value\".to_string(), NativeValue::String(message));\n    NativeValue::Variant {{ name: \"Err\".to_string(), fields }}\n}}\n\n\
fn __some(value: NativeValue) -> NativeValue {{\n    let mut fields = BTreeMap::new();\n    fields.insert(\"value\".to_string(), value);\n    NativeValue::Variant {{ name: \"Some\".to_string(), fields }}\n}}\n\n\
fn __none() -> NativeValue {{\n    NativeValue::Variant {{ name: \"None\".to_string(), fields: BTreeMap::new() }}\n}}\n\n\
{functions}\n\
#[unsafe(no_mangle)]\npub extern \"C\" fn rss_native_registry() -> NativeRegistry {{\n    let entries: Vec<NativeBindingEntry> = vec![\n{entries}    ];\n    let boxed = entries.into_boxed_slice();\n    let len = boxed.len();\n    let ptr = Box::leak(boxed).as_ptr();\n    NativeRegistry {{ entries: ptr, len }}\n}}\n",
    );

    ShimCrate {
        cargo_toml,
        lib_rs,
    }
}

fn render_shim_function(fn_name: &str, binding: &ShimBinding) -> String {
    let symbol = &binding.symbol;
    let mut body = String::new();
    body.push_str("    let mut __args = args.into_iter();\n");
    let mut call_args = Vec::new();
    for (index, param) in binding.params.iter().enumerate() {
        let name = format!("__p{index}");
        let owned = param.owned_rust_ty();
        let conv = param.from_native_expr(&format!("__raw{index}"));
        let is_mut = binding.mut_indices.contains(&index);
        let mut_kw = if is_mut { "mut " } else { "" };
        body.push_str(&format!(
            "    let __raw{index} = __args.next().ok_or_else(|| \"{symbol}: missing argument {index}\".to_string())?;\n"
        ));
        body.push_str(&format!(
            "    let {mut_kw}{name}: {owned} = ({conv}).map_err(|error| format!(\"{symbol} argument {index}: {{error}}\"))?;\n"
        ));
        if is_mut {
            call_args.push(format!("&mut {name}"));
        } else {
            call_args.push(param.call_arg(&name));
        }
    }
    let call = format!("{}({})", binding.rust_path, call_args.join(", "));

    // Build `__result` (a NativeValue) from the native call.
    match &binding.ret {
        ShimReturn::Result(ok_ty) => {
            let ok_conv = ok_ty.to_native_expr("__value");
            body.push_str(&format!(
                "    let __result = match {call} {{\n        Ok(__value) => __ok({ok_conv}),\n        Err(__error) => __err(__error),\n    }};\n"
            ));
        }
        ShimReturn::Plain(ty) => {
            let conv = ty.to_native_expr("__value");
            body.push_str(&format!("    let __value = {call};\n    let __result = {conv};\n"));
        }
    }

    if binding.mut_indices.is_empty() {
        body.push_str("    Ok(__result)\n");
    } else {
        // Envelope: result followed by the mutated `mut` params, in order.
        let mutated = binding
            .mut_indices
            .iter()
            .map(|index| binding.params[*index].to_native_expr(&format!("__p{index}")))
            .collect::<Vec<_>>()
            .join(", ");
        body.push_str(&format!(
            "    Ok(NativeValue::List(vec![__result, {mutated}]))\n"
        ));
    }
    format!("fn {fn_name}(args: Vec<NativeValue>) -> Result<NativeValue, String> {{\n{body}}}\n")
}
