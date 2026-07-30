use std::fs;
use std::path::Path;

const REMOVED_ROOT_ALIASES: &[&str] = &[
    "VmExecutable",
    "vm_compile_source",
    "eval_package_main_with_args",
    "eval_package_main_with_args_and_native_bindings",
    "eval_package_main_with_args_and_native_bindings_and_limits",
    "eval_package_main_with_args_and_native_bindings_streaming_stdout",
    "eval_source_main",
    "eval_source_main_with_args",
    "vm_eval_source_main_with_args",
    "eval_source_main_with_args_and_native_bindings",
    "eval_source_main_with_args_streaming_stdout",
];

fn library_source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("rsscript library source should be readable")
}

fn braced_module<'a>(source: &'a str, declaration: &str) -> &'a str {
    let declaration_start = source
        .find(declaration)
        .unwrap_or_else(|| panic!("missing module declaration `{declaration}`"));
    let body_start = source[declaration_start..]
        .find('{')
        .map(|offset| declaration_start + offset)
        .unwrap_or_else(|| panic!("missing module body for `{declaration}`"));
    let mut depth = 0usize;

    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[declaration_start..=body_start + offset];
                }
            }
            _ => {}
        }
    }

    panic!("unterminated module body for `{declaration}`");
}

#[test]
fn v1_facade_exposes_the_named_domains() {
    use rsscript::api::v1::{diagnostics, frontend, package, review, vm};

    let _analyze = frontend::analyze_source;
    let _format = diagnostics::format_diagnostics_json;
    let _review = review::review_sources;
    let _package = package::package_sources;
    let _compile = vm::reg_vm_compile_source;
    let _limits = vm::VmLimits::safe_default();
}

#[test]
fn removed_root_aliases_cannot_return() {
    let source = library_source();
    let root_exports = source
        .split("/// Versioned, stable entrypoints for embedding RSScript.")
        .next()
        .expect("root export section should precede the versioned facade");
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

#[test]
fn v1_facade_uses_only_explicit_reexports() {
    let source = library_source();
    let facade = braced_module(&source, "pub mod api");
    let required_domains = [
        "pub mod frontend",
        "pub mod diagnostics",
        "pub mod review",
        "pub mod package",
        "pub mod vm",
    ];

    for domain in required_domains {
        assert!(
            facade.contains(domain),
            "api::v1 must retain the explicit `{domain}` domain"
        );
    }
    assert!(
        !facade.contains('*'),
        "api::v1 must use explicit item lists instead of broad facade globs"
    );
}
