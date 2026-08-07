use std::fs;
use std::path::Path;

#[test]
fn runtime_public_facade_has_no_glob_reexports() {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in fs::read_dir(source_dir).expect("runtime source directory should be readable") {
        let path = entry
            .expect("runtime source entry should be readable")
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("runtime source should be readable");
        for (line_index, line) in source.lines().enumerate() {
            assert!(
                !(line.contains("pub use") && line.contains("::*")),
                "{}:{} contains a blanket public re-export",
                path.display(),
                line_index + 1
            );
        }
    }
}

#[test]
fn canonical_facades_exclude_compatibility_entrypoints() {
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("runtime facade should be readable");
    let canonical = source
        .split_once("/// Host integration APIs")
        .expect("host facade marker should exist")
        .1;

    assert!(
        !source.contains("pub mod api"),
        "the removed api::v1 compatibility facade must not return"
    );

    for obsolete in [
        "unwrap_runtime_or_panic",
        "unwrap_runtime,",
        "path_safe_relative",
        "path_resolve_relative",
        "_with_resources",
    ] {
        assert!(
            !canonical.contains(obsolete),
            "canonical facade exposes obsolete entrypoint `{obsolete}`"
        );
    }
}

#[test]
fn runtime_services_are_explicit_and_process_global_lookup_is_absent() {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(!source_dir.join("compatibility.rs").exists());
    for module in ["lib.rs", "operation_context.rs", "async_runtime.rs"] {
        let source = fs::read_to_string(source_dir.join(module))
            .unwrap_or_else(|error| panic!("{module} should be readable: {error}"));
        for forbidden in [
            "OnceLock",
            "generated_abi_runtime_services",
            "default_runtime_services",
        ] {
            assert!(
                !source.contains(forbidden),
                "{module} contains global runtime lookup `{forbidden}`"
            );
        }
    }
    let operation = fs::read_to_string(source_dir.join("operation_context.rs"))
        .expect("operation context source");
    assert!(operation.contains("services: Arc<RuntimeServices>"));
}

#[test]
fn concrete_host_modules_are_absent() {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for module in [
        "domain.rs",
        "env.rs",
        "fs.rs",
        "network/mod.rs",
        "process.rs",
        "random.rs",
        "socket.rs",
        "tempdir.rs",
        "websocket.rs",
    ] {
        assert!(
            !source_dir.join(module).exists(),
            "legacy host module `{module}` must stay outside the AOT runtime"
        );
    }
}
