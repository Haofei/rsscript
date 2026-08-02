use std::fs;
use std::path::Path;

const REMOVED_ROOT_ALIASES: &[&str] = &[
    "VmExecutable",
    "vm_compile_source",
    "eval_package_main_with_args",
    "eval_package_main_with_args_and_external_bindings",
    "eval_package_main_with_args_and_external_bindings_and_limits",
    "eval_package_main_with_args_and_external_bindings_streaming_stdout",
    "eval_source_main",
    "eval_source_main_with_args",
    "vm_eval_source_main_with_args",
    "eval_source_main_with_args_and_external_bindings",
    "eval_source_main_with_args_streaming_stdout",
];

fn library_source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("rsscript library source should be readable")
}

#[test]
fn versioned_facade_is_deleted() {
    let source = library_source();
    assert!(!source.contains("pub mod api"));
    assert!(!source.contains("pub mod v1"));
}

#[test]
fn removed_root_aliases_cannot_return() {
    let source = library_source();
    let root_exports = source.as_str();
    let identifiers = root_exports
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    let violations = REMOVED_ROOT_ALIASES
        .iter()
        .filter(|alias| identifiers.contains(alias))
        .copied()
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "removed compatibility aliases were reintroduced at the crate root: {}",
        violations.join(", ")
    );
}
