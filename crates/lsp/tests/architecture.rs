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
