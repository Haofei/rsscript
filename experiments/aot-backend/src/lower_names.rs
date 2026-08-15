//! Generated-Rust name projection owned by the experimental AOT backend.
//!
//! Core keeps the public inventory projection; this copy intentionally owns
//! the Rust-specific scoped override state so AOT lowering no longer reaches
//! into compiler implementation modules.

use std::collections::HashMap;

use rsscript_syntax::ast::{Item, Program};

thread_local! {
    static LOWER_NAME_OVERRIDES: std::cell::RefCell<HashMap<String, String>> =
        std::cell::RefCell::new(HashMap::new());
}

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

pub(crate) fn rust_qualified_function_ident(namespace: &str, name: &str) -> String {
    let source_name = format!("{namespace}.{}", rsscript_text::type_root_name(name));
    if let Some(pinned) = lower_name_override(&source_name) {
        return pinned;
    }
    namespace
        .split('.')
        .chain(std::iter::once(rsscript_text::type_root_name(name)))
        .map(rust_path_segment)
        .collect::<Vec<_>>()
        .join("_")
}

pub(crate) fn rust_path_segment(segment: &str) -> String {
    if let Some((head, tail)) = segment.split_once("::<") {
        format!("{}::<{tail}", rust_ident(head))
    } else {
        rust_ident(segment)
    }
}

pub(crate) fn rust_ident(name: &str) -> String {
    if rsscript_text::is_rust_keyword(name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}
