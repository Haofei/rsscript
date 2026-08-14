//! Backend symbol-name projection shared by execution metadata and Rust/AOT
//! lowering.
//!
//! This module deliberately owns only source-to-backend name projection and
//! `#lower_name` pins. It is not an AOT implementation: execution inventory
//! tools need the same stable projection without compiling the Rust lowerer.

use std::collections::HashMap;

use crate::syntax::ast::{Item, Program};

thread_local! {
    /// Per-operation map from a function's source-qualified name (for example
    /// `helpers.count`) to its pinned backend name from `#lower_name("...")`.
    static LOWER_NAME_OVERRIDES: std::cell::RefCell<HashMap<String, String>> =
        std::cell::RefCell::new(HashMap::new());
}

/// The backend symbol name for a source-qualified RSScript name. Dotted member
/// names are flattened and Rust keywords are raw-escaped.
pub fn lowered_symbol_name(qualified_name: &str) -> String {
    rust_function_ident(qualified_name)
}

/// Collect the `#lower_name("...")` pins declared by a program.
pub(crate) fn collect_lower_name_overrides(program: &Program) -> HashMap<String, String> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => function
                .lower_name
                .as_ref()
                .map(|pinned| (function.name.clone(), pinned.clone())),
            _ => None,
        })
        .collect()
}

/// Install operation-local name pins and return the map that must be restored
/// when the operation completes.
pub(crate) fn set_lower_name_overrides(
    overrides: HashMap<String, String>,
) -> HashMap<String, String> {
    LOWER_NAME_OVERRIDES.with(|cell| cell.replace(overrides))
}

pub(crate) fn lower_name_override(source_name: &str) -> Option<String> {
    LOWER_NAME_OVERRIDES.with(|cell| cell.borrow().get(source_name).cloned())
}

pub(crate) fn rust_function_ident(name: &str) -> String {
    if let Some(pinned) = lower_name_override(name) {
        return pinned;
    }
    let joined = name.split('.').collect::<Vec<_>>().join("_");
    rust_ident(&joined)
}

#[cfg(feature = "aot-rust")]
pub(crate) fn rust_qualified_function_ident(namespace: &str, name: &str) -> String {
    let source_name = format!("{namespace}.{}", type_root_name(name));
    if let Some(pinned) = lower_name_override(&source_name) {
        return pinned;
    }
    namespace
        .split('.')
        .chain(std::iter::once(type_root_name(name)))
        .map(rust_path_segment)
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(feature = "aot-rust")]
pub(crate) fn rust_path_segment(segment: &str) -> String {
    if let Some((head, tail)) = segment.split_once("::<") {
        format!("{}::<{tail}", rust_ident(head))
    } else {
        rust_ident(segment)
    }
}

pub(crate) fn rust_ident(name: &str) -> String {
    if crate::text_util::is_rust_keyword(name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

#[cfg(feature = "aot-rust")]
fn type_root_name(name: &str) -> &str {
    crate::text_util::type_root_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_available_without_the_aot_lowerer() {
        assert_eq!(lowered_symbol_name("MultiBuffer.ref"), "MultiBuffer_ref");
        assert_eq!(lowered_symbol_name("gen"), "r#gen");
    }
}
