use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::diagnostic::{Diagnostic, Span, code};
use crate::formatter::format_program;
use crate::syntax::ast::{FunctionDecl, Item, Program, TypeRef};
use crate::syntax::parse_source;

use super::super::contract::collect_package_function_contracts;
use super::super::{
    ManifestNativeRust, PackageReviewFileKind, PackageSource, read_utf8_file_bounded,
};
use super::NATIVE_MANIFEST_MAX_BYTES;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeBindingsManifest {
    schema: String,
    #[serde(default)]
    function: Vec<FunctionBinding>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionBinding {
    symbol: String,
    provider: String,
    entry: String,
    #[serde(default)]
    review_effects: Vec<String>,
}

pub(in crate::package) fn package_external_bindings(
    package_dir: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let path = package_dir.join("native/bindings.rssbind.toml");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = read_utf8_file_bounded(
        &path,
        NATIVE_MANIFEST_MAX_BYTES,
        "native binding manifest read",
    )?;
    parse_external_bindings(&path, &source)
}

fn parse_external_bindings(path: &Path, source: &str) -> Result<BTreeMap<String, String>, String> {
    let manifest: NativeBindingsManifest = toml::from_str(source)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    flatten_external_bindings(manifest).map_err(|error| format!("{}: {error}", path.display()))
}

/// Validate the v1 binding descriptor and flatten it to `symbol -> provider::entry`.
fn flatten_external_bindings(
    manifest: NativeBindingsManifest,
) -> Result<BTreeMap<String, String>, String> {
    if manifest.schema != "rsscript.bindings.v1" {
        return Err(format!(
            "unsupported binding schema `{}`; expected `rsscript.bindings.v1`",
            manifest.schema
        ));
    }
    let mut flat = BTreeMap::new();
    for binding in manifest.function {
        let symbol = binding.symbol.trim();
        let provider = binding.provider.trim();
        let entry = binding.entry.trim();
        if symbol.is_empty() || provider.is_empty() || entry.is_empty() {
            return Err("binding symbol, provider, and entry must be non-empty".to_string());
        }
        if binding
            .review_effects
            .iter()
            .any(|effect| effect.trim().is_empty())
        {
            return Err(format!("binding `{symbol}` has an empty review effect"));
        }
        if flat
            .insert(symbol.to_string(), format!("{provider}::{entry}"))
            .is_some()
        {
            return Err(format!("external binding `{symbol}` is defined twice"));
        }
    }
    Ok(flat)
}

pub(in crate::package) fn native_binding_interface_sources(
    sources: &[PackageSource],
    external_bindings: &BTreeMap<String, String>,
) -> Vec<PackageSource> {
    if external_bindings.is_empty() {
        return Vec::new();
    }
    let source_type_names = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .flat_map(|source| parse_source(&source.path, &source.contents).items)
        .filter_map(|item| match item {
            Item::Type(type_decl) => Some(type_decl.name),
            Item::Function(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect::<BTreeSet<_>>();

    sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .filter_map(|source| {
            let mut selected_items = Vec::new();
            for item in parse_source(&source.path, &source.contents).items {
                match item {
                    Item::Type(type_decl) if !source_type_names.contains(&type_decl.name) => {
                        selected_items.push(Item::Type(type_decl));
                    }
                    Item::Function(function) if external_bindings.contains_key(&function.name) => {
                        selected_items.push(Item::Function(function));
                    }
                    _ => {}
                }
            }
            if !selected_items
                .iter()
                .any(|item| matches!(item, Item::Function(_)))
            {
                return None;
            }
            let program = Program {
                unknown_top_level_spans: Vec::new(),
                malformed_declaration_spans: Vec::new(),
                protocols: Vec::new(),
                protocol_impls: Vec::new(),
                items: selected_items,
            };
            Some(PackageSource {
                path: format!("{}#native-bindings", source.path),
                relative_path: format!("{}#native-bindings", source.relative_path),
                contents: format_program(&program),
                kind: PackageReviewFileKind::Interface,
            })
        })
        .collect()
}

pub(in crate::package) fn package_native_binding_diagnostics(
    package_dir: &Path,
    sources: &[PackageSource],
    external_bindings: &BTreeMap<String, String>,
    native: Option<&ManifestNativeRust>,
) -> Vec<Diagnostic> {
    if external_bindings.is_empty() {
        return Vec::new();
    }
    let interface_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let interface_functions = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .flat_map(|source| parse_source(&source.path, &source.contents).items)
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let crate_name = native
        .and_then(|native| native.crate_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let mut diagnostics = Vec::new();
    let native_enabled = native.is_some_and(|native| native.enabled);

    if !native_enabled {
        diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_NATIVE_BINDING,
                "native bindings require enabled `[native.rust]` configuration.",
                native_binding_manifest_span(package_dir),
                "native binding without native Rust wrapper",
            )
            .with_cause("A binding manifest maps RSScript native contracts to Rust wrapper functions, so the package must enable a native Rust wrapper crate.")
            .with_fix(
                "enable_native_rust",
                "Add `[native.rust] enabled = true` with a wrapper crate, or remove `native/bindings.rssbind.toml`.",
                "manual",
            ),
        );
    } else if crate_name.is_none() {
        diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_NATIVE_BINDING,
                "native bindings require `[native.rust].crate`.",
                native_binding_manifest_span(package_dir),
                "native binding crate missing",
            )
            .with_cause("Generated Rust must know which native crate owns the binding targets.")
            .with_fix(
                "declare_native_crate",
                "Set `[native.rust] crate = \"...\"` to the Rust wrapper crate name.",
                "manual",
            ),
        );
    }

    for (symbol, target) in external_bindings {
        let span = native_binding_span(package_dir, symbol);
        if symbol.trim().is_empty() || target.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    "native binding entries must have non-empty RSScript symbols and Rust targets.",
                    span,
                    "invalid native binding",
                )
                .with_cause("Binding keys name RSScript native functions; values name Rust wrapper functions.")
                .with_fix(
                    "fix_native_binding",
                    "Write a binding such as `\"Native.echo\" = \"rss_json_native::echo\"`.",
                    "manual",
                ),
            );
            continue;
        }

        let Some(_contract) = interface_function_contracts.get(symbol) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("external binding `{symbol}` does not match any package interface function."),
                    span,
                    "unknown external binding symbol",
                )
                .with_cause("Bindings must resolve to a bodyless function declared in a package `.rssi` contract.")
                .with_fix(
                    "declare_external_interface",
                    format!("Declare `fn {symbol}(...)` in the package interface, or remove this binding."),
                    "manual",
                ),
            );
            continue;
        };

        if let Some(function) = interface_functions.get(symbol)
            && let Some(reason) = unsupported_native_binding_signature(function)
        {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` uses a type unsupported by the native value bridge: {reason}."),
                    span.clone(),
                    "unsupported native binding type",
                )
                .with_cause("Unsupported native signatures cannot be represented by the generated VM shim and must fail during package checking instead of disappearing at runtime.")
                .with_fix(
                    "use_supported_native_binding_type",
                    "Use Unit, String, Int, Float, Bool, Bytes, Path, List<T>, Option<T>, or Result<T, String> with supported nested T types.",
                    "manual",
                ),
            );
        }

        if let Some(crate_name) = crate_name
            && !target.starts_with(&format!("{crate_name}::"))
        {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!(
                        "native binding `{symbol}` targets `{target}`, outside configured native crate `{crate_name}`."
                    ),
                    span,
                    "native binding crate mismatch",
                )
                .with_cause("The generated Cargo package only wires the configured native Rust crate as a dependency for this package.")
                .with_fix(
                    "use_configured_native_crate",
                    format!("Use a Rust path starting with `{crate_name}::`, or update `[native.rust].crate`."),
                    "manual",
                ),
            );
        }
    }

    diagnostics
}

fn unsupported_native_binding_signature(function: &FunctionDecl) -> Option<String> {
    for param in &function.params {
        if let Some(reason) = unsupported_native_value_type(&param.ty) {
            return Some(format!("parameter `{}` ({reason})", param.name));
        }
    }
    let Some(return_ty) = function.return_ty.as_ref() else {
        return None;
    };
    if return_ty.name == "Result" {
        if return_ty.args.len() != 2
            || return_ty.args[1].name != "String"
            || !return_ty.args[1].args.is_empty()
        {
            return Some("return type must have the shape Result<T, String>".to_string());
        }
        return unsupported_native_value_type(&return_ty.args[0])
            .map(|reason| format!("Result success type ({reason})"));
    }
    unsupported_native_value_type(return_ty).map(|reason| format!("return type ({reason})"))
}

fn unsupported_native_value_type(ty: &TypeRef) -> Option<String> {
    match ty.name.as_str() {
        "Unit" | "String" | "Int" | "Float" | "Bool" | "Bytes" | "Path" if ty.args.is_empty() => {
            None
        }
        "List" | "Option" if ty.args.len() == 1 => unsupported_native_value_type(&ty.args[0]),
        "List" | "Option" => Some(format!("{} requires exactly one type argument", ty.name)),
        _ => Some(format!("type `{}` is not supported", ty.name)),
    }
}

fn native_binding_span(package_dir: &Path, symbol: &str) -> Span {
    let path = package_dir.join("native/bindings.rssbind.toml");
    let file = path.display().to_string();
    let source = read_utf8_file_bounded(
        &path,
        NATIVE_MANIFEST_MAX_BYTES,
        "native binding manifest diagnostic read",
    )
    .unwrap_or_default();
    for (index, line) in source.lines().enumerate() {
        if let Some(column) = line.find(symbol) {
            return Span {
                file,
                line: index + 1,
                column: column + 1,
                length: symbol.len().max(1),
            };
        }
    }
    Span {
        file,
        line: 1,
        column: 1,
        length: symbol.len().max(1),
    }
}

fn native_binding_manifest_span(package_dir: &Path) -> Span {
    Span {
        file: package_dir
            .join("native/bindings.rssbind.toml")
            .display()
            .to_string(),
        line: 1,
        column: 1,
        length: 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(toml_src: &str) -> Result<BTreeMap<String, String>, String> {
        let manifest: NativeBindingsManifest = toml::from_str(toml_src).expect("manifest parses");
        flatten_external_bindings(manifest)
    }

    #[test]
    fn v1_function_bindings_flatten_to_provider_entries() {
        let flat = flatten(
            "schema = \"rsscript.bindings.v1\"\n\n[[function]]\nsymbol = \"NativeOps.sum_int\"\nprovider = \"rss_native_ops\"\nentry = \"sum_int\"\n\n[[function]]\nsymbol = \"NativeOps.sort_int\"\nprovider = \"rss_native_ops\"\nentry = \"sort_int\"\n",
        )
        .expect("v1 descriptor flattens");
        assert_eq!(
            flat.get("NativeOps.sum_int").map(String::as_str),
            Some("rss_native_ops::sum_int")
        );
        assert_eq!(
            flat.get("NativeOps.sort_int").map(String::as_str),
            Some("rss_native_ops::sort_int")
        );
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn duplicate_symbol_errors() {
        let error = flatten(
            "schema = \"rsscript.bindings.v1\"\n\n[[function]]\nsymbol = \"File.open\"\nprovider = \"file_native\"\nentry = \"open\"\n\n[[function]]\nsymbol = \"File.open\"\nprovider = \"file_native\"\nentry = \"open_again\"\n",
        )
        .expect_err("duplicate symbol should error");
        assert!(error.contains("File.open"), "error: {error}");
    }

    #[test]
    fn old_schema_and_empty_fields_error() {
        assert!(
            flatten("schema = \"rsscript.bindings.v0\"\n").is_err(),
            "old schema must error"
        );
        assert!(
            flatten("schema = \"rsscript.bindings.v1\"\n\n[[function]]\nsymbol = \"File.open\"\nprovider = \"\"\nentry = \"open\"\n").is_err(),
            "empty provider must error"
        );
    }
}
