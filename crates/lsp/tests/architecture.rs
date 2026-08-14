use std::fs;
use std::path::{Path, PathBuf};

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read(name: &str) -> String {
    let path = source_root().join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn lsp_depends_on_the_language_service_boundary() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("LSP manifest should be readable");
    assert!(manifest.contains("rsscript-language-service"));
    assert!(manifest.contains("rsscript-compiler"));
    assert!(manifest.contains("rsscript-workspace-loader"));
    assert!(
        !manifest.lines().any(|line| line.starts_with("rsscript =")),
        "the LSP must not depend directly on the product façade"
    );

    for forbidden in [
        "rsscript-aot-runtime",
        "rsscript-runtime",
        "vm-jit",
        "reir",
        "rss-native-abi",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "the LSP manifest must not depend on `{forbidden}`"
        );
    }
}

#[test]
fn lsp_composes_the_transitional_compiler_diagnostic_adapter() {
    let diagnostics = read("diagnostics.rs");
    assert!(diagnostics.contains("workspace_diagnostics"));
    assert!(diagnostics.contains("fn compiler_workspace_diagnostics"));
    assert_eq!(
        diagnostics
            .matches("rsscript_compiler::analyze_frontend_input_snapshot_with_operation")
            .count(),
        1,
        "the LSP may use the compiler only through its single session-snapshot adapter"
    );
    for forbidden in [
        "analyze_source_with_interfaces_result_with_operation",
        "analyze_sources_with_interfaces_result_with_operation",
        ".interfaces()",
        ".sources()",
    ] {
        assert!(
            !diagnostics.contains(forbidden),
            "the LSP diagnostic adapter must forward its immutable session snapshot, not reconstruct `{forbidden}`"
        );
    }
    assert!(
        !diagnostics.contains("lint_source("),
        "LSP diagnostics must leave local linting to language service"
    );
}

#[test]
fn main_is_only_the_lsp_composition_root() {
    let main = read("main.rs");
    assert!(main.lines().count() <= 40, "LSP main.rs must stay small");
    for forbidden in [
        "struct Backend",
        "impl LanguageServer",
        "struct Document",
        "fn semantic_tokens_for_source",
    ] {
        assert!(
            !main.contains(forbidden),
            "LSP main.rs must not own `{forbidden}`"
        );
    }
}

#[test]
fn stateful_lsp_types_have_single_module_owners() {
    let owners = [
        ("DocumentStore", "documents.rs"),
        ("PackageInputCache", "workspace.rs"),
        ("DiagnosticsPublisher", "publication.rs"),
        ("AnalysisJob", "documents.rs"),
        ("Backend", "backend.rs"),
    ];
    let files = fs::read_dir(source_root())
        .expect("LSP source directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect::<Vec<_>>();

    for (type_name, owner) in owners {
        let definition = format!("struct {type_name}");
        let defining_files = files
            .iter()
            .filter(|path| {
                fs::read_to_string(path).is_ok_and(|source| source.contains(&definition))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            defining_files.len(),
            1,
            "`{type_name}` must have exactly one owner"
        );
        assert!(
            defining_files[0].ends_with(owner),
            "`{type_name}` must be owned by {owner}"
        );
    }
}
