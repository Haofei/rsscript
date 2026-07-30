use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::diagnostic::{Diagnostic, Span, code};
use crate::formatter::format_program;
use crate::syntax::ast::{
    EffectDecl, FileFeature, FileFeatureScope, FunctionDecl, Item, Program, TypeRef,
};
use crate::syntax::parse_source;

use super::super::contract::collect_package_function_contracts;
use super::super::{
    ManifestNativeRust, PackageReviewFileKind, PackageSource, read_utf8_file_bounded,
};
use super::NATIVE_MANIFEST_MAX_BYTES;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeBindingsManifest {
    #[serde(default)]
    bindings: BTreeMap<String, String>,
    /// Compact whole-boundary binding: one `[adapter.<Namespace>]` section binds
    /// many `Namespace.method` native functions to `<crate>::<method>` without a
    /// per-function line. Expands into `bindings` at load time, so every
    /// downstream consumer (lowering, VM shim, conformance checks) sees the same
    /// flat map - there is no separate adapter code path.
    #[serde(default)]
    adapter: BTreeMap<String, AdapterBinding>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterBinding {
    /// The Rust wrapper crate that owns every method in this boundary.
    #[serde(rename = "crate")]
    crate_name: String,
    /// Method names declared under the namespace (the part after the dot). Each
    /// `m` binds `Namespace.m` to `<crate>::m`.
    #[serde(default)]
    functions: Vec<String>,
    /// Optional per-method Rust name overrides for `m` whose Rust function name
    /// differs from the RSScript method name (`Namespace.m -> <crate>::<rename[m]>`).
    #[serde(default)]
    rename: BTreeMap<String, String>,
}

pub(in crate::package) fn package_native_bindings(
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
    parse_native_bindings(&path, &source)
}

fn parse_native_bindings(path: &Path, source: &str) -> Result<BTreeMap<String, String>, String> {
    let manifest: NativeBindingsManifest = toml::from_str(source)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    flatten_native_bindings(manifest).map_err(|error| format!("{}: {error}", path.display()))
}

/// Flatten a binding manifest into the canonical `symbol -> rust target` map,
/// expanding any compact `[adapter.*]` sections. Errors on malformed adapters and
/// on a symbol produced by more than one source (explicit binding or adapter), so
/// the binding surface stays unambiguous for review.
fn flatten_native_bindings(
    manifest: NativeBindingsManifest,
) -> Result<BTreeMap<String, String>, String> {
    let mut flat = manifest.bindings;
    for (namespace, adapter) in manifest.adapter {
        let crate_name = adapter.crate_name.trim();
        if crate_name.is_empty() {
            return Err(format!(
                "adapter `{namespace}` must set a non-empty `crate`."
            ));
        }
        if adapter.functions.is_empty() {
            return Err(format!(
                "adapter `{namespace}` lists no `functions`; add the method names it binds or remove the section."
            ));
        }
        for method in &adapter.functions {
            let method = method.trim();
            if method.is_empty() {
                return Err(format!("adapter `{namespace}` has an empty function name."));
            }
            let symbol = format!("{namespace}.{method}");
            let rust_method = adapter
                .rename
                .get(method)
                .map(String::as_str)
                .unwrap_or(method);
            let target = format!("{crate_name}::{rust_method}");
            if flat.insert(symbol.clone(), target).is_some() {
                return Err(format!(
                    "native binding `{symbol}` is defined twice (by `[adapter.{namespace}]` and an explicit `[bindings]` entry, or duplicated)."
                ));
            }
        }
    }
    Ok(flat)
}

pub(in crate::package) fn native_binding_interface_sources(
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
) -> Vec<PackageSource> {
    if native_bindings.is_empty() {
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
                    Item::Function(function)
                        if function
                            .effects
                            .contains(&EffectDecl::Name("native".to_string()))
                            && native_bindings.contains_key(&function.name) =>
                    {
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
                features: vec![FileFeature::Native],
                feature_scopes: vec![FileFeatureScope {
                    file: source.path.clone(),
                    features: vec![FileFeature::Native],
                }],
                unknown_features: Vec::new(),
                duplicate_features: Vec::new(),
                feature_spans: Vec::new(),
                profile_spans: Vec::new(),
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
    native_bindings: &BTreeMap<String, String>,
    native: Option<&ManifestNativeRust>,
) -> Vec<Diagnostic> {
    if native_bindings.is_empty() {
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

    for (symbol, target) in native_bindings {
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

        let Some(contract) = interface_function_contracts.get(symbol) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` does not match any package interface function."),
                    span,
                    "unknown native binding symbol",
                )
                .with_cause("Native bindings are reviewable only when their RSScript side is declared in a package `.rssi` contract.")
                .with_fix(
                    "declare_native_interface",
                    format!("Declare `native fn {symbol}(...)` in the package interface, or remove this binding."),
                    "manual",
                ),
            );
            continue;
        };

        if !contract.effects.contains("native") {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_NATIVE_BINDING,
                    format!("native binding `{symbol}` points to a non-native interface function."),
                    span.clone(),
                    "non-native binding symbol",
                )
                .with_cause("Only interface functions declared with the native boundary can be implemented by native wrapper bindings.")
                .with_fix(
                    "mark_native_interface",
                    format!("Declare `{symbol}` as `native fn` or add `effects(native)`, or remove this binding."),
                    "manual",
                ),
            );
        }

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
        flatten_native_bindings(manifest)
    }

    #[test]
    fn adapter_section_expands_to_per_method_bindings() {
        let flat = flatten(
            "[adapter.Rayon]\ncrate = \"rss_rayon_native\"\nfunctions = [\"sum_int\", \"sort_int\"]\n",
        )
        .expect("adapter expands");
        assert_eq!(
            flat.get("Rayon.sum_int").map(String::as_str),
            Some("rss_rayon_native::sum_int")
        );
        assert_eq!(
            flat.get("Rayon.sort_int").map(String::as_str),
            Some("rss_rayon_native::sort_int")
        );
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn adapter_form_matches_the_explicit_form() {
        let compact = flatten(
            "[adapter.Rayon]\ncrate = \"rss_rayon_native\"\nfunctions = [\"sum_int\", \"sort_int\"]\n",
        )
        .unwrap();
        let explicit = flatten(
            "[bindings]\n\"Rayon.sum_int\" = \"rss_rayon_native::sum_int\"\n\"Rayon.sort_int\" = \"rss_rayon_native::sort_int\"\n",
        )
        .unwrap();
        assert_eq!(compact, explicit);
    }

    #[test]
    fn rename_overrides_the_rust_method_name() {
        let flat = flatten(
            "[adapter.File]\ncrate = \"file_native\"\nfunctions = [\"open\"]\n\n[adapter.File.rename]\nopen = \"open_file\"\n",
        )
        .unwrap();
        assert_eq!(
            flat.get("File.open").map(String::as_str),
            Some("file_native::open_file")
        );
    }

    #[test]
    fn explicit_bindings_and_adapters_compose() {
        let flat = flatten(
            "[bindings]\n\"Extra.ping\" = \"extra::ping\"\n\n[adapter.File]\ncrate = \"file_native\"\nfunctions = [\"open\"]\n",
        )
        .unwrap();
        assert_eq!(flat.len(), 2);
        assert!(flat.contains_key("Extra.ping"));
        assert!(flat.contains_key("File.open"));
    }

    #[test]
    fn duplicate_symbol_across_adapter_and_bindings_errors() {
        let error = flatten(
            "[bindings]\n\"File.open\" = \"file_native::open\"\n\n[adapter.File]\ncrate = \"file_native\"\nfunctions = [\"open\"]\n",
        )
        .expect_err("duplicate symbol should error");
        assert!(error.contains("File.open"), "error: {error}");
    }

    #[test]
    fn empty_crate_and_empty_functions_error() {
        assert!(
            flatten("[adapter.File]\ncrate = \"\"\nfunctions = [\"open\"]\n").is_err(),
            "empty crate must error"
        );
        assert!(
            flatten("[adapter.File]\ncrate = \"file_native\"\nfunctions = []\n").is_err(),
            "empty functions must error"
        );
    }
}
