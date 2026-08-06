//! Dynamically loads the native host bindings a package needs to run in the VM.
//!
//! For a package with native dependencies, this reads the binding symbols and
//! their RSS signatures, generates a cdylib shim crate (see [`super::shim_gen`]),
//! builds it with cargo, `dlopen`s the result, and pulls out a
//! `(symbol, ExternalFunction)` table the VM can call. Nothing is hard-coded
//! per package: the shim is derived entirely from the package's interface and
//! `bindings.rssbind.toml`.
//!
//! Cache use is fail-closed on platforms where this crate cannot verify private
//! owner/ACL enforcement. Such platforms need a dedicated secure-cache backend
//! before native shim loading can be enabled.

use std::collections::BTreeMap;
use std::path::Path;

use crate::eval_types::{
    BlockingBehavior, CancellationBehavior, ExternalFunction, ExternalFunctionRegistry,
    ExternalSymbol, FunctionSignature, ProviderCallMode, ProviderDescriptor, ProviderFunction,
    ProviderFunctionDescriptor,
};
use rsscript_abi_model::{DataEffect as AbiDataEffect, ParameterSignature};

use crate::package::{ExecutablePackageSnapshot, prepare_package_for_execution};
use crate::syntax::ast::{DataEffect, FunctionDecl, Item, TypeRef};
use crate::syntax::parse_source;

use super::shim_gen::ShimDependency;

mod cache;
mod shim;

use shim::{build_binding, build_shim_library};

/// Load (generating and building the shim cdylib on demand) the native host
/// bindings for `package_dir`. Returns an empty list for pure-RSS packages.
///
/// The loaded library is leaked so the returned function pointers stay valid for
/// the lifetime of the process.
pub fn load_package_bindings(
    package_dir: &Path,
) -> Result<Vec<(String, ExternalFunction)>, String> {
    let prepared = prepare_package_for_execution(package_dir)?;
    if !prepared.requires_external_provider() {
        return Ok(Vec::new());
    }
    let package = prepared.verify()?;
    load_package_bindings_from_snapshot(&package)
}

/// Build and load native bindings for a previously reviewed package.
///
/// This is the only entry point that can reach Cargo or `dlopen`; its argument
/// cannot be constructed outside the package authorization service.
pub fn load_package_bindings_from_snapshot(
    package: &ExecutablePackageSnapshot,
) -> Result<Vec<(String, ExternalFunction)>, String> {
    let input = package.lowering_input();
    if input.native_dependencies.is_empty() {
        return Ok(Vec::new());
    }
    let snapshot_root = package.native_snapshot_root().ok_or_else(|| {
        "native build denied: authorized package has no private content snapshot".to_string()
    })?;
    let abi_path = package.native_abi_path().ok_or_else(|| {
        "native build denied: authorized package has no ABI content snapshot".to_string()
    })?;

    // Collect the native-fn signatures declared across the package interfaces.
    let mut signatures: BTreeMap<String, FunctionDecl> = BTreeMap::new();
    for (path, contents) in &input.interfaces {
        let program = parse_source(path, contents);
        for item in &program.items {
            if let Item::Function(decl) = item {
                signatures.insert(decl.name.clone(), decl.clone());
            }
        }
    }

    // Build the shim binding specs plus the set of native crate deps to link.
    let mut bindings = Vec::new();
    let mut native_deps = Vec::new();
    for dependency in package.native_build_dependencies() {
        native_deps.push(ShimDependency {
            crate_name: dependency.crate_name.clone(),
            path: dependency.path.clone(),
            cargo_features: dependency.cargo_features.clone(),
            default_features: dependency.default_features,
        });
        for (symbol, rust_path) in &dependency.bindings {
            let declaration = signatures.get(symbol).ok_or_else(|| {
                format!("native binding `{symbol}` has no interface signature for the VM shim.")
            })?;
            bindings.push(build_binding(
                symbol,
                rust_path,
                &declaration.params,
                declaration.return_ty.as_ref(),
            )?);
        }
    }
    native_deps.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then(left.path.cmp(&right.path))
            .then(left.cargo_features.cmp(&right.cargo_features))
            .then(left.default_features.cmp(&right.default_features))
    });
    native_deps.dedup();
    bindings.sort_by(|left, right| left.symbol.cmp(&right.symbol));

    let library_path = build_shim_library(snapshot_root, abi_path, &native_deps, &bindings)?;
    let loaded = rss_native_abi::load_registry(&library_path)?;
    validate_loaded_provider(package, &signatures, &bindings, loaded)
}

fn validate_loaded_provider(
    package: &ExecutablePackageSnapshot,
    signatures: &BTreeMap<String, FunctionDecl>,
    bindings: &[super::shim_gen::ShimBinding],
    loaded: Vec<(String, ExternalFunction)>,
) -> Result<Vec<(String, ExternalFunction)>, String> {
    let functions = bindings
        .iter()
        .map(|binding| {
            let declaration = signatures.get(&binding.symbol).ok_or_else(|| {
                format!(
                    "native binding `{}` lost its interface signature before provider linking",
                    binding.symbol
                )
            })?;
            let symbol = ExternalSymbol::new(binding.symbol.clone())
                .map_err(|_| format!("invalid external symbol `{}`", binding.symbol))?;
            Ok(ProviderFunctionDescriptor {
                symbol,
                signature: semantic_signature(declaration),
                entry: binding.rust_path.clone(),
                call_mode: if declaration.is_async {
                    ProviderCallMode::Async
                } else {
                    ProviderCallMode::Sync
                },
                blocking: BlockingBehavior::MayBlock,
                cancellation: if declaration.is_async {
                    CancellationBehavior::Cooperative
                } else {
                    CancellationBehavior::NotApplicable
                },
                thread_safe: true,
                reentrant: false,
                resource_cleanup_contract: "RSScript interface resource contract".to_string(),
                error_mapping: "native ABI result string".to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let descriptor = ProviderDescriptor {
        provider_id: format!("native.{}", package.lowering_input().package.name),
        provider_version: package.lowering_input().package.version.clone(),
        supported_abi: vec![rss_native_abi::ABI_VERSION],
        functions,
    };
    let mut implementations = BTreeMap::new();
    for (symbol, callable) in loaded {
        let symbol = ExternalSymbol::new(symbol.clone())
            .map_err(|_| format!("native provider exported invalid symbol `{symbol}`"))?;
        let declaration = signatures
            .get(symbol.as_str())
            .ok_or_else(|| format!("native provider exported undeclared symbol `{symbol}`"))?;
        implementations.insert(
            symbol,
            ProviderFunction {
                signature: semantic_signature(declaration),
                callable,
            },
        );
    }
    let mut registry = ExternalFunctionRegistry::new();
    registry
        .register_provider(&descriptor, implementations)
        .map_err(|error| error.to_string())?;
    Ok(registry.into_bindings().collect())
}

fn semantic_signature(declaration: &FunctionDecl) -> FunctionSignature {
    FunctionSignature {
        parameters: declaration
            .params
            .iter()
            .map(|parameter| ParameterSignature {
                name: parameter.name.clone(),
                effect: match parameter.effect.unwrap_or(DataEffect::Read) {
                    DataEffect::Read => AbiDataEffect::Read,
                    DataEffect::Mut => AbiDataEffect::Mut,
                    DataEffect::Take => AbiDataEffect::Take,
                },
                type_name: signature_type_name(&parameter.ty),
                retained: declaration.retained_params.contains(&parameter.name),
            })
            .collect(),
        return_type: declaration
            .return_ty
            .as_ref()
            .map_or_else(|| "Unit".to_string(), signature_type_name),
        asynchronous: declaration.is_async,
    }
}

fn signature_type_name(ty: &TypeRef) -> String {
    let body = if ty.name == "Fn" {
        let parameters = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let effect = ty
                    .fn_param_effects
                    .get(index)
                    .copied()
                    .flatten()
                    .unwrap_or(DataEffect::Read)
                    .as_str();
                format!("{effect} {}", signature_type_name(parameter))
            })
            .collect::<Vec<_>>()
            .join(",");
        let result = ty
            .fn_return
            .as_ref()
            .map_or_else(|| "Unit".to_string(), |result| signature_type_name(result));
        format!("Fn({parameters})->{result}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(signature_type_name)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let body = if ty.is_owned {
        format!("owned {body}")
    } else if ty.is_noescape {
        format!("noescape {body}")
    } else {
        body
    };
    if ty.is_fresh {
        format!("fresh {body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    //! The native-plugin loader's *signature → shim binding* mapping. The build +
    //! `dlopen` half is OS plumbing (libloading) exercised by the gated e2e test;
    //! the bug-prone logic is this type mapping, which was previously 0% covered.
    //! Inputs are parsed from tiny interface snippets so we don't hand-build spans.
    use std::fs;

    use rss_native_abi::NativeValue;
    use sha2::{Digest, Sha256};

    use super::super::shim_gen::{ShimBinding, ShimReturn, ShimType};
    use super::cache::{create_private_dir, hash_file_streaming_bounded, open_private_lock};
    use super::*;
    use crate::syntax::ast::Item;
    use crate::syntax::ast::{Param, TypeRef};
    use crate::syntax::parse_source;

    fn sig(src: &str) -> (Vec<Param>, Option<TypeRef>) {
        let program = parse_source("t.rssi", src);
        for item in program.items {
            if let Item::Function(decl) = item {
                return (decl.params, decl.return_ty);
            }
        }
        panic!("interface snippet declared no function");
    }

    #[test]
    fn streaming_hash_rejects_bytes_beyond_scanned_size() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-hash-growth-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("growing.rs");
        fs::write(&path, b"12345").expect("fixture file");
        let mut digest = Sha256::new();

        let error = hash_file_streaming_bounded(&path, &mut digest, 4)
            .expect_err("hashing beyond scanned size must fail");
        let _ = fs::remove_dir_all(&root);
        assert!(
            error.contains("approved byte limit of 4"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn path_loader_rejects_package_before_reaching_native_loading() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-authorization-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory");
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"authorization-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[native.rust]\nenabled = true\npath = \"native/rust\"\ncrate = \"authorization_test_native\"\nbuild_scripts = \"forbid\"\nproc_macros = \"forbid\"\nunsafe = \"forbid\"\n",
        )
        .expect("fixture manifest");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("fixture source");
        fs::create_dir_all(root.join("native/rust/src")).expect("fixture native source directory");
        fs::write(
            root.join("native/rust/Cargo.toml"),
            "[package]\nname = \"authorization_test_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture native manifest");
        fs::write(root.join("native/rust/src/lib.rs"), "pub fn unused() {}\n")
            .expect("fixture native source");
        fs::write(
            root.join("native/rust/Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"authorization_test_native\"\nversion = \"0.1.0\"\n",
        )
        .expect("fixture reviewed Cargo lock");

        let error = match load_package_bindings(&root) {
            Ok(_) => {
                panic!("a package without an approved lock/check must not authorize native load")
            }
            Err(error) => error,
        };
        let _ = fs::remove_dir_all(&root);

        assert!(error.contains("native build/load denied"), "{error}");
        assert!(error.contains("rsspkg.lock missing"), "{error}");
    }

    #[test]
    fn builds_plain_scalar_binding() {
        let (params, ret) = sig("fn Adder.add(a: Int, b: Int) -> Int\n");
        let binding = build_binding("Adder.add", "adder::add", &params, ret.as_ref())
            .expect("scalar binding should build");
        assert_eq!(binding.symbol, "Adder.add");
        assert_eq!(binding.rust_path, "adder::add");
        assert_eq!(binding.params, vec![ShimType::Int, ShimType::Int]);
        assert_eq!(binding.ret, ShimReturn::Plain(ShimType::Int));
        assert!(binding.mut_indices.is_empty());
    }

    #[test]
    fn records_mut_param_positions() {
        let (params, ret) = sig("fn Buf.fill(buffer: mut Bytes, value: Int) -> Unit\n");
        let binding = build_binding("Buf.fill", "buf::fill", &params, ret.as_ref())
            .expect("mut binding should build");
        assert_eq!(binding.mut_indices, vec![0]);
        assert_eq!(binding.params, vec![ShimType::Bytes, ShimType::Int]);
    }

    #[test]
    fn maps_result_and_nested_container_types() {
        let (params, ret) =
            sig("fn P.run(items: read List<String>) -> Result<Option<Int>, String>\n");
        let binding = build_binding("P.run", "p::run", &params, ret.as_ref())
            .expect("container binding should build");
        assert_eq!(
            binding.params,
            vec![ShimType::List(Box::new(ShimType::String))]
        );
        assert_eq!(
            binding.ret,
            ShimReturn::Result(ShimType::Option(Box::new(ShimType::Int)))
        );
    }

    #[test]
    fn missing_return_type_is_plain_unit() {
        let (params, ret) = sig("fn X.g(a: Int)\n");
        let binding = build_binding("X.g", "x::g", &params, ret.as_ref())
            .expect("unit-return binding should build");
        assert_eq!(binding.ret, ShimReturn::Plain(ShimType::Unit));
    }

    #[test]
    fn rejects_unsupported_param_type() {
        let (params, ret) = sig("fn X.f(cfg: read Config) -> Unit\n");
        let error = build_binding("X.f", "x::f", &params, ret.as_ref())
            .expect_err("unsupported parameter must fail shim construction");
        assert!(error.contains("parameter `cfg` has unsupported type `Config`"));
    }

    #[test]
    fn rejects_unsupported_return_type() {
        let (params, ret) = sig("fn X.h(a: Int) -> Config\n");
        let error = build_binding("X.h", "x::h", &params, ret.as_ref())
            .expect_err("unsupported return must fail shim construction");
        assert!(error.contains("unsupported return type `Config`"));
    }

    #[test]
    fn rejects_result_with_non_string_error_type() {
        let (params, ret) = sig("fn X.h(a: Int) -> Result<Int, Config>\n");
        let error = build_binding("X.h", "x::h", &params, ret.as_ref())
            .expect_err("unsupported Result error must fail shim construction");
        assert!(error.contains("Result<T, String>"));
    }

    #[cfg(unix)]
    #[test]
    fn private_cache_directory_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rss-native-cache-symlink-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let real = root.join("real");
        fs::create_dir_all(&real).expect("real directory should create");
        let linked = root.join("linked");
        symlink(&real, &linked).expect("cache symlink should create");

        let error = create_private_dir(&linked).expect_err("symlinked cache must be rejected");
        let _ = fs::remove_dir_all(&root);

        assert!(error.contains("not a symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn private_cache_lock_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "rss-native-cache-lock-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        create_private_dir(&root).expect("cache directory should create");
        let target = root.join("target");
        fs::write(&target, "do not lock").expect("target should write");
        let linked = root.join("entry.lock");
        symlink(&target, &linked).expect("lock symlink should create");

        let error = open_private_lock(&linked).expect_err("symlinked lock must be rejected");
        let _ = fs::remove_dir_all(&root);

        assert!(error.contains("regular file, not a symlink"));
    }

    #[cfg(not(unix))]
    #[test]
    fn private_cache_fails_closed_without_acl_backend() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-cache-platform-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        let error = create_private_dir(&root)
            .expect_err("private cache must require a supported ACL backend");
        assert!(
            error.contains("private owner and ACL enforcement")
                || error.contains("platform backend is unavailable"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn shim_build_is_atomic_and_uses_versioned_byte_abi() {
        let root = std::env::temp_dir().join(format!(
            "rss-native-shim-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let native = root.join("native");
        fs::create_dir_all(native.join("src")).expect("native source directory should create");
        fs::write(
            native.join("Cargo.toml"),
            "[package]\nname = \"shim_test_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .expect("native manifest should write");
        fs::write(
            native.join("src/lib.rs"),
            "pub fn add_one(value: i64) -> i64 { value + 1 }\n",
        )
        .expect("native source should write");
        fs::write(
            native.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"shim_test_native\"\nversion = \"0.1.0\"\n",
        )
        .expect("reviewed native lock should write");
        let deps = vec![ShimDependency {
            crate_name: "shim_test_native".to_string(),
            path: native.to_string_lossy().into_owned(),
            cargo_features: vec![],
            default_features: true,
        }];
        let bindings = vec![ShimBinding {
            symbol: "Demo.add_one".to_string(),
            rust_path: "shim_test_native::add_one".to_string(),
            params: vec![ShimType::Int],
            ret: ShimReturn::Plain(ShimType::Int),
            mut_indices: vec![],
        }];

        let abi = Path::new(env!("CARGO_MANIFEST_DIR")).join("../native-abi");
        let paths = std::thread::scope(|scope| {
            let first = scope.spawn(|| build_shim_library(&root, &abi, &deps, &bindings));
            let second = scope.spawn(|| build_shim_library(&root, &abi, &deps, &bindings));
            [
                first.join().expect("first build thread should not panic"),
                second.join().expect("second build thread should not panic"),
            ]
        });
        let first = paths[0].as_ref().expect("first shim build should pass");
        let second = paths[1].as_ref().expect("second shim build should pass");
        assert_eq!(first, second);

        let loaded = rss_native_abi::load_registry(first).expect("registry should load");
        let (_, function) = loaded
            .iter()
            .find(|(name, _)| name == "Demo.add_one")
            .expect("generated binding should exist");
        assert_eq!(
            function.call(vec![NativeValue::Int(4)]),
            Ok(NativeValue::Int(5))
        );
        drop(loaded);
        fs::remove_dir_all(&root).expect("test source should clean up");
    }
}
