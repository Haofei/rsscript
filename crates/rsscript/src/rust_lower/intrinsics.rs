use super::*;

use crate::syntax::ast::Callee;
use crate::text_util::type_root_name;

pub(super) fn runtime_intrinsic_target(callee: &Callee) -> Option<&'static str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    runtime_abi::lookup_runtime_intrinsic(type_root_name(namespace), type_root_name(name))
        .map(|intrinsic| intrinsic.rust_target)
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

pub(super) fn is_file_open_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && type_root_name(name) == "open")
}

pub(super) fn is_async_runtime_intrinsic_callee(callee: &Callee) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    let (namespace, name) = (type_root_name(namespace), type_root_name(name));
    matches!(
        (namespace, name),
        ("Timer", "sleep" | "sleep_until" | "sleep_cancellable")
            | (
                "File",
                "read_all_async" | "read_all_string_async" | "write_async" | "write_string_async",
            )
            | (
                "Http",
                "get_async"
                    | "get_timeout_async"
                    | "get_retry_async"
                    | "send_async"
                    | "post_form_async"
                    | "post_json_async"
                    | "post_json_timeout_async"
                    | "post_json_retry_async"
                    | "post_json_bearer_retry_async",
            )
            | (
                "Process",
                "run_async"
                    | "run_timeout_async"
                    | "run_request_async"
                    | "run_request_cancellable_async"
                    | "run_stdout_async"
                    | "run_stdout_timeout_async"
                    | "run_many_stdout_async"
                    | "run_many_stdout_timeout_async",
            )
            | ("Sender", "send" | "send_cancellable")
            | ("Receiver", "recv" | "recv_cancellable")
            | ("Stream", "next")
            | ("Tcp", "connect")
            | ("TcpStream", "read" | "write" | "write_all" | "shutdown")
            | (
                "WebSocket",
                "connect" | "send_text" | "send_bytes" | "recv_text" | "recv_bytes" | "close"
            )
    )
}

pub(super) fn is_file_open_read_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && type_root_name(name) == "open_read")
}

pub(super) fn is_file_open_write_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && type_root_name(name) == "open_write")
}

pub(super) fn is_resource_pool_borrow_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "borrow")
}

pub(super) fn is_resource_pool_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "new")
}

pub(super) fn is_resource_pool_try_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_new")
}

pub(super) fn is_resource_pool_lazy_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "lazy")
}

pub(super) fn is_resource_pool_try_lazy_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_lazy")
}

pub(super) fn is_resource_pool_try_borrow_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_borrow")
}
