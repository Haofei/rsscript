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
