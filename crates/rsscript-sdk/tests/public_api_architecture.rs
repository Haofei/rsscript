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

fn inventory() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/architecture/sdk-api-inventory.md"),
    )
    .expect("SDK public API inventory should be readable")
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

#[test]
fn public_api_inventory_covers_the_current_migration_surface() {
    let inventory = inventory();
    for required in [
        "## Stable façade",
        "## Compatibility-only APIs",
        "## Feature-gated experimental APIs",
        "`reg_vm_*`",
        "`native-jit`",
    ] {
        assert!(
            inventory.contains(required),
            "SDK API inventory must classify `{required}`"
        );
    }

    let source = library_source();
    for module in [
        "pub mod compile",
        "pub mod operation",
        "pub mod artifact",
        "pub mod provider_api",
        "pub mod runtime",
        "pub mod report",
        "pub mod analysis",
    ] {
        assert!(
            source.contains(module),
            "stable façade module `{module}` is missing"
        );
    }

    for forbidden in [
        "pub use rsscript_vm::JitPlan",
        "pub use rsscript_vm::RegInstr",
    ] {
        assert!(
            !source.contains(forbidden),
            "experimental VM detail must not enter the default SDK surface: `{forbidden}`"
        );
    }
    assert!(
        source.contains("#[cfg(feature = \"native-jit\")]\npub use rsscript_vm::NativeStats"),
        "native JIT statistics must remain feature-gated"
    );
    assert!(
        source.contains("#[cfg(feature = \"native-jit\")]\npub use vm_adapter"),
        "native JIT execution helpers must remain feature-gated"
    );
    for legacy_export in [
        "pub use rsscript_compiler::{",
        "pub use rsscript_bytecode::{",
        "pub use rsscript_vm::{",
        "pub use vm_adapter::{",
    ] {
        let gated = format!("#[cfg(feature = \"compatibility\")]\n{legacy_export}");
        assert!(
            source.contains(&gated),
            "legacy root export must require the compatibility feature: {legacy_export}"
        );
    }
}
