use super::*;

use crate::syntax::ast::Callee;
use crate::text_util::type_root_name;

pub(super) fn runtime_intrinsic_target(callee: &Callee) -> Option<&'static str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    rsscript_aot_backend::runtime_intrinsic_target(type_root_name(namespace), type_root_name(name))
}

pub(super) fn runtime_intrinsic_wants_managed_handle_arg(
    callee: &Callee,
    arg_name: Option<&str>,
) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    let Some(arg_name) = arg_name else {
        return false;
    };
    runtime_abi::lookup_runtime_intrinsic(type_root_name(namespace), type_root_name(name))
        .is_some_and(|intrinsic| intrinsic.managed_handle_args.contains(&arg_name))
}

/// The RSS surface treats scalar collection keys and values as ordinary values,
/// while the Rust collection runtime borrows them so it can support non-Copy
/// keys uniformly. Keep that ABI fact here, next to intrinsic resolution,
/// instead of relying on source-level `read` markers at every call site.
pub(super) fn runtime_intrinsic_borrows_arg(
    callee: &Callee,
    arg_name: Option<&str>,
    index: usize,
) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    runtime_collection_intrinsic_borrows_arg(
        type_root_name(namespace),
        type_root_name(name),
        arg_name,
        index,
    )
}

pub(super) fn runtime_collection_intrinsic_borrows_arg(
    namespace: &str,
    name: &str,
    arg_name: Option<&str>,
    index: usize,
) -> bool {
    let position = |expected: &str, expected_index: usize| {
        arg_name == Some(expected) || (arg_name.is_none() && index == expected_index)
    };
    match (namespace, name) {
        ("Map", "contains_key" | "get" | "remove") => position("key", 1),
        ("Map", "get_or_default") => position("key", 1) || position("default", 2),
        ("Map", "insert" | "insert_old") => position("key", 1) || position("value", 2),
        ("Set", "contains" | "insert" | "remove") => position("value", 1),
        ("Option", "unwrap_or") => position("default", 1),
        ("Result", "unwrap_or") => position("default", 1),
        _ => false,
    }
}

pub(super) fn is_string_concat_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "String" && type_root_name(name) == "concat")
}

pub(super) fn is_async_runtime_intrinsic_callee(callee: &Callee) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    let (namespace, name) = (type_root_name(namespace), type_root_name(name));
    matches!(
        (namespace, name),
        ("Sender", "send" | "send_cancellable")
            | ("Receiver", "recv" | "recv_cancellable")
            | ("Stream", "next")
    )
}
