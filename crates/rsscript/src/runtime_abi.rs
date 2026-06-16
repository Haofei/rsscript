pub(crate) struct RuntimeIntrinsic {
    pub(crate) rust_target: &'static str,
    pub(crate) managed_handle_args: &'static [&'static str],
    namespace: &'static str,
    name: &'static str,
}

const fn runtime_intrinsic(
    namespace: &'static str,
    name: &'static str,
    rust_target: &'static str,
) -> RuntimeIntrinsic {
    RuntimeIntrinsic {
        namespace,
        name,
        rust_target,
        managed_handle_args: &[],
    }
}

const fn runtime_intrinsic_with_handles(
    namespace: &'static str,
    name: &'static str,
    rust_target: &'static str,
    managed_handle_args: &'static [&'static str],
) -> RuntimeIntrinsic {
    RuntimeIntrinsic {
        namespace,
        name,
        rust_target,
        managed_handle_args,
    }
}

pub(crate) fn lookup_runtime_intrinsic(
    namespace: &str,
    name: &str,
) -> Option<&'static RuntimeIntrinsic> {
    RUNTIME_INTRINSICS
        .iter()
        .find(|intrinsic| intrinsic.namespace == namespace && intrinsic.name == name)
}

pub(crate) fn runtime_intrinsic_signatures() -> Vec<String> {
    RUNTIME_INTRINSICS
        .iter()
        .map(|intrinsic| format!("{}.{}", intrinsic.namespace, intrinsic.name))
        .collect()
}

pub(crate) fn runtime_intrinsic_supported_signatures() -> Vec<String> {
    let runtime_functions = runtime_public_function_names();
    RUNTIME_INTRINSICS
        .iter()
        .filter_map(|intrinsic| {
            let target = intrinsic.rust_target.strip_prefix("rsscript_runtime::")?;
            runtime_functions
                .contains(target)
                .then(|| format!("{}.{}", intrinsic.namespace, intrinsic.name))
        })
        .collect()
}

fn runtime_public_function_names() -> std::collections::HashSet<String> {
    let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
    let mut functions = std::collections::HashSet::new();
    for entry in std::fs::read_dir(&runtime_src).expect("runtime/src should be readable") {
        let path = entry.expect("runtime/src entry should be readable").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("runtime source should be readable");
        collect_runtime_public_functions(&source, &mut functions);
    }
    functions
}

fn collect_runtime_public_functions(
    source: &str,
    functions: &mut std::collections::HashSet<String>,
) {
    let bytes = source.as_bytes();
    let mut index = 0;
    while let Some(relative) = source[index..].find("pub ") {
        index += relative + "pub ".len();
        let mut cursor = skip_ascii_whitespace(bytes, index);
        if source[cursor..].starts_with("async ") {
            cursor += "async ".len();
            cursor = skip_ascii_whitespace(bytes, cursor);
        }
        if !source[cursor..].starts_with("fn ") {
            continue;
        }
        cursor += "fn ".len();
        cursor = skip_ascii_whitespace(bytes, cursor);
        let start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor > start {
            functions.insert(source[start..cursor].to_string());
        }
        index = cursor;
    }
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

const RUNTIME_INTRINSICS: &[RuntimeIntrinsic] = &[
    runtime_intrinsic("Assert", "equal", "rsscript_runtime::assert_equal"),
    runtime_intrinsic(
        "Assert",
        "equal_bool",
        "rsscript_runtime::assert_equal_bool",
    ),
    runtime_intrinsic("Assert", "equal_int", "rsscript_runtime::assert_equal_int"),
    runtime_intrinsic("Base64", "decode", "rsscript_runtime::base64_decode"),
    runtime_intrinsic(
        "Base64",
        "decode_string",
        "rsscript_runtime::base64_decode_string",
    ),
    runtime_intrinsic("Base64", "encode", "rsscript_runtime::base64_encode"),
    runtime_intrinsic(
        "Base64",
        "encode_bytes",
        "rsscript_runtime::base64_encode_bytes",
    ),
    runtime_intrinsic("Buffer", "clear", "rsscript_runtime::buffer_clear"),
    runtime_intrinsic("Buffer", "consume", "rsscript_runtime::buffer_consume"),
    runtime_intrinsic("Buffer", "is_empty", "rsscript_runtime::buffer_is_empty"),
    runtime_intrinsic("Buffer", "len", "rsscript_runtime::buffer_len"),
    runtime_intrinsic("Buffer", "new", "rsscript_runtime::buffer_new"),
    runtime_intrinsic("Buffer", "view", "rsscript_runtime::buffer_view"),
    runtime_intrinsic(
        "BufferView",
        "is_empty",
        "rsscript_runtime::buffer_view_is_empty",
    ),
    runtime_intrinsic("BufferView", "len", "rsscript_runtime::buffer_view_len"),
    runtime_intrinsic("BufferView", "slice", "rsscript_runtime::buffer_view_slice"),
    runtime_intrinsic(
        "BufferView",
        "to_bytes",
        "rsscript_runtime::buffer_view_to_bytes",
    ),
    runtime_intrinsic("Bytes", "concat", "rsscript_runtime::bytes_concat"),
    runtime_intrinsic("Bytes", "consume", "rsscript_runtime::bytes_consume"),
    runtime_intrinsic(
        "Bytes",
        "from_buffer",
        "rsscript_runtime::bytes_from_buffer",
    ),
    runtime_intrinsic(
        "Bytes",
        "from_string",
        "rsscript_runtime::bytes_from_string",
    ),
    runtime_intrinsic("Bytes", "from_uints", "rsscript_runtime::bytes_from_uints"),
    runtime_intrinsic("Bytes", "is_empty", "rsscript_runtime::bytes_is_empty"),
    runtime_intrinsic("Bytes", "len", "rsscript_runtime::bytes_len"),
    runtime_intrinsic("Bytes", "slice", "rsscript_runtime::bytes_slice"),
    runtime_intrinsic("Bytes", "to_string", "rsscript_runtime::bytes_to_string"),
    runtime_intrinsic("Bytes", "to_uints", "rsscript_runtime::bytes_to_uints"),
    runtime_intrinsic("Bytes", "view", "rsscript_runtime::bytes_view"),
    runtime_intrinsic(
        "BytesView",
        "is_empty",
        "rsscript_runtime::bytes_view_is_empty",
    ),
    runtime_intrinsic("BytesView", "len", "rsscript_runtime::bytes_view_len"),
    runtime_intrinsic("BytesView", "slice", "rsscript_runtime::bytes_view_slice"),
    runtime_intrinsic(
        "BytesView",
        "starts_with",
        "rsscript_runtime::bytes_view_starts_with",
    ),
    runtime_intrinsic(
        "BytesView",
        "to_bytes",
        "rsscript_runtime::bytes_view_to_bytes",
    ),
    runtime_intrinsic("Cache", "get", "rsscript_runtime::cache_get"),
    runtime_intrinsic("Cache", "insert", "rsscript_runtime::cache_insert"),
    runtime_intrinsic("Cache", "lookup", "rsscript_runtime::cache_lookup"),
    runtime_intrinsic("Cache", "new", "rsscript_runtime::cache_new"),
    runtime_intrinsic("Config", "load", "rsscript_runtime::config_load"),
    runtime_intrinsic("Config", "name", "rsscript_runtime::config_name"),
    runtime_intrinsic("Config", "new", "rsscript_runtime::config_new"),
    runtime_intrinsic(
        "Config",
        "rule_count",
        "rsscript_runtime::config_rule_count",
    ),
    runtime_intrinsic("ConfigStore", "name", "rsscript_runtime::config_store_name"),
    runtime_intrinsic("ConfigStore", "new", "rsscript_runtime::config_store_new"),
    runtime_intrinsic(
        "ConfigStore",
        "replace",
        "rsscript_runtime::config_store_replace",
    ),
    runtime_intrinsic("Counter", "add", "rsscript_runtime::counter_add"),
    runtime_intrinsic("Counter", "new", "rsscript_runtime::counter_new"),
    runtime_intrinsic("Counter", "value", "rsscript_runtime::counter_value"),
    runtime_intrinsic("Clock", "now", "rsscript_runtime::clock_now"),
    runtime_intrinsic(
        "Clock",
        "system_unix_ms",
        "rsscript_runtime::clock_system_unix_ms",
    ),
    runtime_intrinsic(
        "Directory",
        "create_all",
        "rsscript_runtime::directory_create_all",
    ),
    runtime_intrinsic(
        "Directory",
        "create_dir_all",
        "rsscript_runtime::directory_create_all",
    ),
    runtime_intrinsic("Directory", "create", "rsscript_runtime::directory_create"),
    runtime_intrinsic(
        "Directory",
        "copy_file",
        "rsscript_runtime::directory_copy_file",
    ),
    runtime_intrinsic("Directory", "exists", "rsscript_runtime::directory_exists"),
    runtime_intrinsic("Directory", "is_dir", "rsscript_runtime::directory_is_dir"),
    runtime_intrinsic(
        "Directory",
        "is_file",
        "rsscript_runtime::directory_is_file",
    ),
    runtime_intrinsic(
        "Directory",
        "list_files",
        "rsscript_runtime::directory_list_files",
    ),
    runtime_intrinsic(
        "Directory",
        "list_paths",
        "rsscript_runtime::directory_list_paths",
    ),
    runtime_intrinsic(
        "Directory",
        "metadata",
        "rsscript_runtime::directory_metadata",
    ),
    runtime_intrinsic(
        "Directory",
        "read_string",
        "rsscript_runtime::directory_read_string",
    ),
    runtime_intrinsic(
        "Directory",
        "remove_dir_all",
        "rsscript_runtime::directory_remove_dir_all",
    ),
    runtime_intrinsic(
        "Directory",
        "remove_file",
        "rsscript_runtime::directory_remove_file",
    ),
    runtime_intrinsic("Directory", "rename", "rsscript_runtime::directory_rename"),
    runtime_intrinsic(
        "Directory",
        "write_string",
        "rsscript_runtime::directory_write_string",
    ),
    runtime_intrinsic("Diff", "unified", "rsscript_runtime::diff_unified"),
    runtime_intrinsic(
        "DecodeError",
        "message",
        "rsscript_runtime::decode_error_message",
    ),
    runtime_intrinsic("Duration", "add", "rsscript_runtime::duration_add"),
    runtime_intrinsic("Duration", "as_ms", "rsscript_runtime::duration_as_ms"),
    runtime_intrinsic(
        "Duration",
        "as_seconds",
        "rsscript_runtime::duration_as_seconds",
    ),
    runtime_intrinsic("Duration", "ms", "rsscript_runtime::duration_ms"),
    runtime_intrinsic("Duration", "seconds", "rsscript_runtime::duration_seconds"),
    runtime_intrinsic(
        "CancellationSource",
        "new",
        "rsscript_runtime::cancellation_source_new",
    ),
    runtime_intrinsic(
        "CancellationSource",
        "token",
        "rsscript_runtime::cancellation_source_token",
    ),
    runtime_intrinsic(
        "CancellationSource",
        "cancel",
        "rsscript_runtime::cancellation_source_cancel",
    ),
    runtime_intrinsic(
        "CancellationToken",
        "is_cancelled",
        "rsscript_runtime::cancellation_token_is_cancelled",
    ),
    runtime_intrinsic("Channel", "bounded", "rsscript_runtime::channel_bounded"),
    // Message channel: same runtime as `bounded`; the cross-isolate payload
    // contract is a compile-time check, not a runtime difference.
    runtime_intrinsic("Channel", "message", "rsscript_runtime::channel_bounded"),
    runtime_intrinsic("Channel", "sender", "rsscript_runtime::channel_sender"),
    runtime_intrinsic("Channel", "receiver", "rsscript_runtime::channel_receiver"),
    runtime_intrinsic("Sender", "send", "rsscript_runtime::sender_send"),
    runtime_intrinsic(
        "Sender",
        "send_cancellable",
        "rsscript_runtime::sender_send_cancellable",
    ),
    runtime_intrinsic("Sender", "close", "rsscript_runtime::sender_close"),
    runtime_intrinsic("Receiver", "recv", "rsscript_runtime::receiver_recv"),
    runtime_intrinsic(
        "Receiver",
        "recv_cancellable",
        "rsscript_runtime::receiver_recv_cancellable",
    ),
    runtime_intrinsic("Receiver", "close", "rsscript_runtime::receiver_close"),
    runtime_intrinsic(
        "Receiver",
        "into_stream",
        "rsscript_runtime::receiver_into_stream",
    ),
    runtime_intrinsic(
        "Stream",
        "collect_list",
        "rsscript_runtime::stream_collect_list",
    ),
    runtime_intrinsic("Stream", "from_list", "rsscript_runtime::stream_from_list"),
    runtime_intrinsic("Stream", "next", "rsscript_runtime::stream_next"),
    runtime_intrinsic("Tcp", "connect", "rsscript_runtime::tcp_connect"),
    runtime_intrinsic("TcpStream", "read", "rsscript_runtime::tcp_stream_read"),
    runtime_intrinsic("TcpStream", "write", "rsscript_runtime::tcp_stream_write"),
    runtime_intrinsic(
        "TcpStream",
        "write_all",
        "rsscript_runtime::tcp_stream_write_all",
    ),
    runtime_intrinsic(
        "TcpStream",
        "shutdown",
        "rsscript_runtime::tcp_stream_shutdown",
    ),
    runtime_intrinsic("TcpError", "message", "rsscript_runtime::tcp_error_message"),
    runtime_intrinsic(
        "WebSocket",
        "connect",
        "rsscript_runtime::websocket_connect",
    ),
    runtime_intrinsic(
        "WebSocket",
        "send_text",
        "rsscript_runtime::websocket_send_text",
    ),
    runtime_intrinsic(
        "WebSocket",
        "send_bytes",
        "rsscript_runtime::websocket_send_bytes",
    ),
    runtime_intrinsic(
        "WebSocket",
        "recv_text",
        "rsscript_runtime::websocket_recv_text",
    ),
    runtime_intrinsic(
        "WebSocket",
        "recv_bytes",
        "rsscript_runtime::websocket_recv_bytes",
    ),
    runtime_intrinsic("WebSocket", "close", "rsscript_runtime::websocket_close"),
    runtime_intrinsic(
        "WebSocketError",
        "message",
        "rsscript_runtime::websocket_error_message",
    ),
    runtime_intrinsic(
        "ChannelError",
        "message",
        "rsscript_runtime::channel_error_message",
    ),
    runtime_intrinsic(
        "Tensor",
        "from_f32_slice",
        "rsscript_runtime::tensor_from_f32_slice",
    ),
    runtime_intrinsic(
        "Tensor",
        "to_f32_slice",
        "rsscript_runtime::tensor_to_f32_slice",
    ),
    runtime_intrinsic("Tensor", "shape", "rsscript_runtime::tensor_shape"),
    runtime_intrinsic("Tensor", "rank", "rsscript_runtime::tensor_rank"),
    // f32 <-> little-endian device-buffer bytes (fused-backend buffer plumbing)
    runtime_intrinsic(
        "Tensor",
        "f32_to_le_bytes",
        "rsscript_runtime::tensor_f32_to_le_bytes",
    ),
    runtime_intrinsic(
        "Tensor",
        "f32_from_le_bytes",
        "rsscript_runtime::tensor_f32_from_le_bytes",
    ),
    runtime_intrinsic("Tensor", "matmul", "rsscript_runtime::tensor_matmul"),
    // Metal GPU compute path
    runtime_intrinsic(
        "Tensor",
        "matmul_metal",
        "rsscript_runtime::tensor_matmul_metal",
    ),
    runtime_intrinsic(
        "Tensor",
        "metal_available",
        "rsscript_runtime::tensor_metal_available",
    ),
    runtime_intrinsic(
        "Tensor",
        "metal_device_name",
        "rsscript_runtime::tensor_metal_device_name",
    ),
    runtime_intrinsic(
        "Tensor",
        "gpu_run_msl",
        "rsscript_runtime::tensor_gpu_run_msl",
    ),
    // `Metal.*` aliases: same native functions under a namespace that does NOT
    // collide with a host program's own `Tensor` type (the tinygrad port defines
    // `struct Tensor`, which shadows the native `Tensor.*` methods).
    runtime_intrinsic("Metal", "available", "rsscript_runtime::tensor_metal_available"),
    runtime_intrinsic(
        "Metal",
        "device_name",
        "rsscript_runtime::tensor_metal_device_name",
    ),
    runtime_intrinsic("Metal", "gpu_run_msl", "rsscript_runtime::tensor_gpu_run_msl"),
    runtime_intrinsic("Tensor", "add", "rsscript_runtime::tensor_add"),
    runtime_intrinsic("Tensor", "sub", "rsscript_runtime::tensor_sub"),
    runtime_intrinsic("Tensor", "mul", "rsscript_runtime::tensor_mul"),
    runtime_intrinsic("Tensor", "div", "rsscript_runtime::tensor_div"),
    runtime_intrinsic("Tensor", "neg", "rsscript_runtime::tensor_neg"),
    runtime_intrinsic("Tensor", "exp", "rsscript_runtime::tensor_exp"),
    runtime_intrinsic("Tensor", "log", "rsscript_runtime::tensor_log"),
    runtime_intrinsic("Tensor", "sqrt", "rsscript_runtime::tensor_sqrt"),
    runtime_intrinsic("Tensor", "relu", "rsscript_runtime::tensor_relu"),
    runtime_intrinsic("Tensor", "sum_all", "rsscript_runtime::tensor_sum_all"),
    runtime_intrinsic("Tensor", "sum_axis", "rsscript_runtime::tensor_sum_axis"),
    runtime_intrinsic("Tensor", "max_axis", "rsscript_runtime::tensor_max_axis"),
    runtime_intrinsic("Tensor", "mean_axis", "rsscript_runtime::tensor_mean_axis"),
    runtime_intrinsic(
        "Tensor",
        "argmax_axis",
        "rsscript_runtime::tensor_argmax_axis",
    ),
    runtime_intrinsic("Tensor", "reshape", "rsscript_runtime::tensor_reshape"),
    runtime_intrinsic(
        "Tensor",
        "transpose",
        "rsscript_runtime::tensor_transpose",
    ),
    runtime_intrinsic("Tensor", "permute", "rsscript_runtime::tensor_permute"),
    runtime_intrinsic(
        "Tensor",
        "broadcast_to",
        "rsscript_runtime::tensor_broadcast_to",
    ),
    runtime_intrinsic("Tensor", "cmplt", "rsscript_runtime::tensor_cmplt"),
    runtime_intrinsic("Tensor", "cmpne", "rsscript_runtime::tensor_cmpne"),
    runtime_intrinsic("Tensor", "cmpeq", "rsscript_runtime::tensor_cmpeq"),
    runtime_intrinsic("Tensor", "select", "rsscript_runtime::tensor_select"),
    runtime_intrinsic("Tensor", "maximum", "rsscript_runtime::tensor_maximum"),
    runtime_intrinsic("Tensor", "minimum", "rsscript_runtime::tensor_minimum"),
    runtime_intrinsic(
        "Tensor",
        "cast_f32",
        "rsscript_runtime::tensor_cast_f32",
    ),
    runtime_intrinsic(
        "Tensor",
        "cast_i32",
        "rsscript_runtime::tensor_cast_i32",
    ),
    runtime_intrinsic(
        "Tensor",
        "cast_bool",
        "rsscript_runtime::tensor_cast_bool",
    ),
    runtime_intrinsic(
        "Tensor",
        "dtype_code",
        "rsscript_runtime::tensor_dtype_code",
    ),
    // movement+gather (ops B)
    runtime_intrinsic("Tensor", "pad", "rsscript_runtime::tensor_pad"),
    runtime_intrinsic("Tensor", "shrink", "rsscript_runtime::tensor_shrink"),
    runtime_intrinsic("Tensor", "flip", "rsscript_runtime::tensor_flip"),
    runtime_intrinsic("Tensor", "gather", "rsscript_runtime::tensor_gather"),
    // reductions+math (ops C)
    runtime_intrinsic(
        "Tensor",
        "prod_axis",
        "rsscript_runtime::tensor_prod_axis",
    ),
    runtime_intrinsic("Tensor", "min_axis", "rsscript_runtime::tensor_min_axis"),
    runtime_intrinsic("Tensor", "sum_axes", "rsscript_runtime::tensor_sum_axes"),
    runtime_intrinsic(
        "Tensor",
        "prod_axes",
        "rsscript_runtime::tensor_prod_axes",
    ),
    runtime_intrinsic("Tensor", "max_axes", "rsscript_runtime::tensor_max_axes"),
    runtime_intrinsic("Tensor", "min_axes", "rsscript_runtime::tensor_min_axes"),
    runtime_intrinsic(
        "Tensor",
        "mean_axes",
        "rsscript_runtime::tensor_mean_axes",
    ),
    runtime_intrinsic(
        "Tensor",
        "reciprocal",
        "rsscript_runtime::tensor_reciprocal",
    ),
    runtime_intrinsic("Tensor", "exp2", "rsscript_runtime::tensor_exp2"),
    runtime_intrinsic("Tensor", "log2", "rsscript_runtime::tensor_log2"),
    runtime_intrinsic("Tensor", "rsqrt", "rsscript_runtime::tensor_rsqrt"),
    runtime_intrinsic("Tensor", "sin", "rsscript_runtime::tensor_sin"),
    runtime_intrinsic("Tensor", "trunc", "rsscript_runtime::tensor_trunc"),
    runtime_intrinsic("Tensor", "pow", "rsscript_runtime::tensor_pow"),
    // bmm+int/bit (ops D)
    runtime_intrinsic("Tensor", "bmm", "rsscript_runtime::tensor_bmm"),
    runtime_intrinsic("Tensor", "idiv", "rsscript_runtime::tensor_idiv"),
    runtime_intrinsic("Tensor", "modulo", "rsscript_runtime::tensor_mod"),
    runtime_intrinsic("Tensor", "floordiv", "rsscript_runtime::tensor_floordiv"),
    runtime_intrinsic("Tensor", "floormod", "rsscript_runtime::tensor_floormod"),
    runtime_intrinsic("Tensor", "shl", "rsscript_runtime::tensor_shl"),
    runtime_intrinsic("Tensor", "shr", "rsscript_runtime::tensor_shr"),
    runtime_intrinsic("Tensor", "bit_and", "rsscript_runtime::tensor_and"),
    runtime_intrinsic("Tensor", "bit_or", "rsscript_runtime::tensor_or"),
    runtime_intrinsic("Tensor", "bit_xor", "rsscript_runtime::tensor_xor"),
    runtime_intrinsic(
        "Tensor",
        "bitcast_f32_to_i32",
        "rsscript_runtime::tensor_bitcast_f32_to_i32",
    ),
    runtime_intrinsic(
        "Tensor",
        "bitcast_i32_to_f32",
        "rsscript_runtime::tensor_bitcast_i32_to_f32",
    ),
    // rng (slice E)
    runtime_intrinsic("Tensor", "rand", "rsscript_runtime::tensor_rand"),
    runtime_intrinsic("Tensor", "randint", "rsscript_runtime::tensor_randint"),
    runtime_intrinsic("Tensor", "randn", "rsscript_runtime::tensor_randn"),
    // nn (slice F)
    runtime_intrinsic("Tensor", "iota", "rsscript_runtime::tensor_iota"),
    runtime_intrinsic("Tensor", "one_hot", "rsscript_runtime::tensor_one_hot"),
    runtime_intrinsic("Tensor", "softmax", "rsscript_runtime::tensor_softmax"),
    runtime_intrinsic(
        "Tensor",
        "log_softmax",
        "rsscript_runtime::tensor_log_softmax",
    ),
    runtime_intrinsic(
        "Tensor",
        "cross_entropy",
        "rsscript_runtime::tensor_cross_entropy",
    ),
    // conv (slice G)
    runtime_intrinsic("Tensor", "conv2d", "rsscript_runtime::tensor_conv2d"),
    runtime_intrinsic(
        "Tensor",
        "max_pool2d",
        "rsscript_runtime::tensor_max_pool2d",
    ),
    runtime_intrinsic(
        "Tensor",
        "avg_pool2d",
        "rsscript_runtime::tensor_avg_pool2d",
    ),
    // scatter
    runtime_intrinsic(
        "Tensor",
        "scatter_add",
        "rsscript_runtime::tensor_scatter_add",
    ),
    runtime_intrinsic(
        "TensorError",
        "message",
        "rsscript_runtime::tensor_error_message",
    ),
    runtime_intrinsic("ResourcePool", "stats", "rsscript_runtime::pool_stats"),
    runtime_intrinsic(
        "ResourcePool",
        "discard",
        "rsscript_runtime::resource_lease_discard",
    ),
    runtime_intrinsic(
        "PoolStats",
        "capacity",
        "rsscript_runtime::pool_stats_capacity",
    ),
    runtime_intrinsic(
        "PoolStats",
        "created",
        "rsscript_runtime::pool_stats_created",
    ),
    runtime_intrinsic(
        "PoolStats",
        "available",
        "rsscript_runtime::pool_stats_available",
    ),
    runtime_intrinsic("PoolStats", "in_use", "rsscript_runtime::pool_stats_in_use"),
    runtime_intrinsic(
        "PoolError",
        "message",
        "rsscript_runtime::pool_error_message",
    ),
    runtime_intrinsic(
        "Deadline",
        "after_ms",
        "rsscript_runtime::deadline_after_ms",
    ),
    runtime_intrinsic("Deadline", "after", "rsscript_runtime::deadline_after"),
    runtime_intrinsic(
        "Deadline",
        "is_expired",
        "rsscript_runtime::deadline_is_expired",
    ),
    runtime_intrinsic(
        "Deadline",
        "remaining_ms",
        "rsscript_runtime::deadline_remaining_ms",
    ),
    runtime_intrinsic("Csv", "open_read", "rsscript_runtime::csv_open_read"),
    runtime_intrinsic("Csv", "parse_row", "rsscript_runtime::csv_parse_row"),
    runtime_intrinsic("Csv", "read_into", "rsscript_runtime::csv_read_into"),
    runtime_intrinsic("Csv", "rows", "rsscript_runtime::csv_rows"),
    runtime_intrinsic("Db", "close", "rsscript_runtime::db_close"),
    runtime_intrinsic(
        "DbConnection",
        "open",
        "rsscript_runtime::db_connection_open",
    ),
    runtime_intrinsic(
        "DbConnection",
        "query",
        "rsscript_runtime::db_connection_query",
    ),
    runtime_intrinsic(
        "DbConnection",
        "try_open",
        "rsscript_runtime::db_connection_try_open",
    ),
    runtime_intrinsic("Date", "add_days", "rsscript_runtime::date_add_days"),
    runtime_intrinsic("Date", "add_ms", "rsscript_runtime::date_add_ms"),
    runtime_intrinsic("Date", "day", "rsscript_runtime::date_day"),
    runtime_intrinsic(
        "Date",
        "days_between",
        "rsscript_runtime::date_days_between",
    ),
    runtime_intrinsic("Date", "format_iso", "rsscript_runtime::date_format_iso"),
    runtime_intrinsic("Date", "format_ymd", "rsscript_runtime::date_format_ymd"),
    runtime_intrinsic("Date", "hour", "rsscript_runtime::date_hour"),
    runtime_intrinsic(
        "Date",
        "days_in_month",
        "rsscript_runtime::date_days_in_month",
    ),
    runtime_intrinsic(
        "Date",
        "is_leap_year",
        "rsscript_runtime::date_is_leap_year",
    ),
    runtime_intrinsic("Date", "minute", "rsscript_runtime::date_minute"),
    runtime_intrinsic("Date", "month", "rsscript_runtime::date_month"),
    runtime_intrinsic("Date", "parse_iso", "rsscript_runtime::date_parse_iso"),
    runtime_intrinsic("Date", "parse_ymd", "rsscript_runtime::date_parse_ymd"),
    runtime_intrinsic("Date", "second", "rsscript_runtime::date_second"),
    runtime_intrinsic(
        "Date",
        "start_of_day",
        "rsscript_runtime::date_start_of_day",
    ),
    runtime_intrinsic("Date", "weekday", "rsscript_runtime::date_weekday"),
    runtime_intrinsic("Date", "year", "rsscript_runtime::date_year"),
    runtime_intrinsic_with_handles(
        "Environment",
        "bind_function",
        "rsscript_runtime::environment_bind_function",
        &["env", "function"],
    ),
    runtime_intrinsic_with_handles(
        "Environment",
        "child",
        "rsscript_runtime::environment_child",
        &["parent"],
    ),
    runtime_intrinsic_with_handles(
        "Environment",
        "has_function",
        "rsscript_runtime::environment_has_function",
        &["env"],
    ),
    runtime_intrinsic_with_handles(
        "Environment",
        "has_parent",
        "rsscript_runtime::environment_has_parent",
        &["env"],
    ),
    runtime_intrinsic("Environment", "root", "rsscript_runtime::environment_root"),
    runtime_intrinsic("Env", "current_dir", "rsscript_runtime::env_current_dir"),
    runtime_intrinsic("Env", "get", "rsscript_runtime::env_get"),
    runtime_intrinsic(
        "Env",
        "get_or_default",
        "rsscript_runtime::env_get_or_default",
    ),
    runtime_intrinsic("Env", "home_dir", "rsscript_runtime::env_home_dir"),
    runtime_intrinsic(
        "Env",
        "run_workspace_root",
        "rsscript_runtime::env_run_workspace_root",
    ),
    runtime_intrinsic("Env", "set", "rsscript_runtime::env_set"),
    runtime_intrinsic(
        "Env",
        "set_current_dir",
        "rsscript_runtime::env_set_current_dir",
    ),
    runtime_intrinsic("Env", "temp_dir", "rsscript_runtime::env_temp_dir"),
    runtime_intrinsic("File", "open", "rsscript_runtime::file_open"),
    runtime_intrinsic("File", "open_read", "rsscript_runtime::file_open_read"),
    runtime_intrinsic("File", "open_write", "rsscript_runtime::file_open_write"),
    runtime_intrinsic("File", "exists", "rsscript_runtime::file_exists"),
    runtime_intrinsic(
        "File",
        "append_bytes",
        "rsscript_runtime::file_append_bytes",
    ),
    runtime_intrinsic(
        "File",
        "append_string",
        "rsscript_runtime::file_append_string",
    ),
    runtime_intrinsic("File", "remove", "rsscript_runtime::file_remove"),
    runtime_intrinsic("File", "read_bytes", "rsscript_runtime::file_read_bytes"),
    runtime_intrinsic("File", "read_string", "rsscript_runtime::file_read_string"),
    runtime_intrinsic("File", "write_bytes", "rsscript_runtime::file_write_bytes"),
    runtime_intrinsic(
        "File",
        "write_string_to_path",
        "rsscript_runtime::file_write_string_to_path",
    ),
    runtime_intrinsic(
        "File",
        "write_atomic",
        "rsscript_runtime::file_write_atomic",
    ),
    runtime_intrinsic("File", "read_all", "rsscript_runtime::file_read_all"),
    runtime_intrinsic(
        "File",
        "read_all_string",
        "rsscript_runtime::file_read_all_string",
    ),
    runtime_intrinsic(
        "File",
        "bytes_stream",
        "rsscript_runtime::file_bytes_stream",
    ),
    runtime_intrinsic("File", "read_into", "rsscript_runtime::file_read_into"),
    runtime_intrinsic("File", "write", "rsscript_runtime::file_write"),
    runtime_intrinsic("File", "write_bytes_view", "rsscript_runtime::file_write"),
    runtime_intrinsic(
        "File",
        "write_string",
        "rsscript_runtime::file_write_string",
    ),
    runtime_intrinsic(
        "File",
        "write_buffer",
        "rsscript_runtime::file_write_buffer",
    ),
    runtime_intrinsic("File", "write_buffer_view", "rsscript_runtime::file_write"),
    runtime_intrinsic(
        "File",
        "read_all_async",
        "rsscript_runtime::file_read_all_async",
    ),
    runtime_intrinsic(
        "File",
        "read_all_string_async",
        "rsscript_runtime::file_read_all_string_async",
    ),
    runtime_intrinsic("File", "write_async", "rsscript_runtime::file_write_async"),
    runtime_intrinsic(
        "File",
        "write_string_async",
        "rsscript_runtime::file_write_string_async",
    ),
    runtime_intrinsic(
        "FileError",
        "message",
        "rsscript_runtime::file_error_message",
    ),
    runtime_intrinsic_with_handles(
        "FunctionObject",
        "has_closure",
        "rsscript_runtime::function_object_has_closure",
        &["function"],
    ),
    runtime_intrinsic_with_handles(
        "FunctionObject",
        "new",
        "rsscript_runtime::function_object_new",
        &["closure"],
    ),
    runtime_intrinsic("GlobalConfig", "new", "rsscript_runtime::global_config_new"),
    runtime_intrinsic(
        "GlobalConfig",
        "replace",
        "rsscript_runtime::global_config_replace",
    ),
    runtime_intrinsic(
        "GlobalConfig",
        "rule_count",
        "rsscript_runtime::global_config_rule_count",
    ),
    runtime_intrinsic(
        "Hash",
        "sha256_bytes",
        "rsscript_runtime::hash_sha256_bytes",
    ),
    runtime_intrinsic(
        "Hash",
        "sha3_224_bytes",
        "rsscript_runtime::hash_sha3_224_bytes",
    ),
    runtime_intrinsic(
        "Hash",
        "sha3_256_bytes",
        "rsscript_runtime::hash_sha3_256_bytes",
    ),
    runtime_intrinsic("Hash", "sha256_file", "rsscript_runtime::hash_sha256_file"),
    runtime_intrinsic(
        "Hash",
        "sha256_string",
        "rsscript_runtime::hash_sha256_string",
    ),
    runtime_intrinsic(
        "Hash",
        "shake128_bytes",
        "rsscript_runtime::hash_shake128_bytes",
    ),
    runtime_intrinsic(
        "Hmac",
        "sha256_bytes",
        "rsscript_runtime::hmac_sha256_bytes",
    ),
    runtime_intrinsic(
        "Hmac",
        "sha256_string",
        "rsscript_runtime::hmac_sha256_string",
    ),
    runtime_intrinsic("Hex", "decode", "rsscript_runtime::hex_decode"),
    runtime_intrinsic("Hex", "encode", "rsscript_runtime::hex_encode"),
    runtime_intrinsic(
        "Hex",
        "encode_string",
        "rsscript_runtime::hex_encode_string",
    ),
    runtime_intrinsic(
        "Gzip",
        "decompress_bytes",
        "rsscript_runtime::gzip_decompress_bytes",
    ),
    runtime_intrinsic("Http", "get", "rsscript_runtime::http_get"),
    runtime_intrinsic("Http", "get_async", "rsscript_runtime::http_get_async"),
    runtime_intrinsic("Http", "send_async", "rsscript_runtime::http_send_async"),
    runtime_intrinsic(
        "Http",
        "get_timeout_async",
        "rsscript_runtime::http_get_timeout_async",
    ),
    runtime_intrinsic(
        "Http",
        "get_retry_async",
        "rsscript_runtime::http_get_retry_async",
    ),
    runtime_intrinsic("Http", "post_form", "rsscript_runtime::http_post_form"),
    runtime_intrinsic(
        "Http",
        "post_form_async",
        "rsscript_runtime::http_post_form_async",
    ),
    runtime_intrinsic("Http", "post_json", "rsscript_runtime::http_post_json"),
    runtime_intrinsic(
        "Http",
        "post_json_async",
        "rsscript_runtime::http_post_json_async",
    ),
    runtime_intrinsic(
        "Http",
        "post_json_timeout_async",
        "rsscript_runtime::http_post_json_timeout_async",
    ),
    runtime_intrinsic(
        "Http",
        "post_json_retry_async",
        "rsscript_runtime::http_post_json_retry_async",
    ),
    runtime_intrinsic(
        "Http",
        "post_json_bearer_retry_async",
        "rsscript_runtime::http_post_json_bearer_retry_async",
    ),
    runtime_intrinsic(
        "HttpError",
        "message",
        "rsscript_runtime::http_error_message",
    ),
    runtime_intrinsic("HttpRequest", "json", "rsscript_runtime::http_request_json"),
    runtime_intrinsic(
        "HttpRequest",
        "with_timeout",
        "rsscript_runtime::http_request_with_timeout",
    ),
    runtime_intrinsic(
        "HttpRequest",
        "with_retry",
        "rsscript_runtime::http_request_with_retry",
    ),
    runtime_intrinsic(
        "HttpRequest",
        "with_header",
        "rsscript_runtime::http_request_with_header",
    ),
    runtime_intrinsic(
        "HttpResponse",
        "is_success",
        "rsscript_runtime::http_response_is_success",
    ),
    runtime_intrinsic(
        "HttpResponse",
        "bytes",
        "rsscript_runtime::http_response_bytes",
    ),
    runtime_intrinsic(
        "HttpResponse",
        "lines",
        "rsscript_runtime::http_response_lines",
    ),
    runtime_intrinsic(
        "HttpResponse",
        "status",
        "rsscript_runtime::http_response_status",
    ),
    runtime_intrinsic(
        "HttpResponse",
        "text",
        "rsscript_runtime::http_response_text",
    ),
    runtime_intrinsic_with_handles(
        "Image",
        "inspect",
        "rsscript_runtime::image_inspect",
        &["image"],
    ),
    runtime_intrinsic("Image", "load", "rsscript_runtime::image_load"),
    runtime_intrinsic("Image", "normalize", "rsscript_runtime::image_normalize"),
    runtime_intrinsic("Image", "resize", "rsscript_runtime::image_resize"),
    runtime_intrinsic_with_handles("Image", "save", "rsscript_runtime::image_save", &["image"]),
    runtime_intrinsic("Image", "sharpen", "rsscript_runtime::image_sharpen"),
    runtime_intrinsic("Instant", "elapsed", "rsscript_runtime::instant_elapsed"),
    runtime_intrinsic(
        "Json",
        "array_contains_prefix",
        "rsscript_runtime::json_array_contains_prefix",
    ),
    runtime_intrinsic(
        "Json",
        "array_contains_string",
        "rsscript_runtime::json_array_contains_string",
    ),
    runtime_intrinsic(
        "Json",
        "array_contains_substring",
        "rsscript_runtime::json_array_contains_substring",
    ),
    runtime_intrinsic(
        "Json",
        "array_count_where",
        "rsscript_runtime::json_array_count_where",
    ),
    runtime_intrinsic("Json", "array_fold", "rsscript_runtime::json_array_fold"),
    runtime_intrinsic("Json", "array_get", "rsscript_runtime::json_array_get"),
    runtime_intrinsic("Json", "array", "rsscript_runtime::json_array"),
    runtime_intrinsic("Json", "array_bools", "rsscript_runtime::json_array_bools"),
    runtime_intrinsic("Json", "array_ints", "rsscript_runtime::json_array_ints"),
    runtime_intrinsic("Json", "array_len", "rsscript_runtime::json_array_len"),
    runtime_intrinsic(
        "Json",
        "array_strings",
        "rsscript_runtime::json_array_strings",
    ),
    runtime_intrinsic("Json", "as_bool", "rsscript_runtime::json_as_bool"),
    runtime_intrinsic("Json", "as_int", "rsscript_runtime::json_as_int"),
    runtime_intrinsic("Json", "as_string", "rsscript_runtime::json_as_string"),
    runtime_intrinsic("Json", "at", "rsscript_runtime::json_at"),
    runtime_intrinsic("Json", "at_bool", "rsscript_runtime::json_at_bool"),
    runtime_intrinsic("Json", "at_bool_or", "rsscript_runtime::json_at_bool_or"),
    runtime_intrinsic("Json", "at_int", "rsscript_runtime::json_at_int"),
    runtime_intrinsic("Json", "at_int_or", "rsscript_runtime::json_at_int_or"),
    runtime_intrinsic("Json", "at_optional", "rsscript_runtime::json_at_optional"),
    runtime_intrinsic(
        "Json",
        "at_optional_bool",
        "rsscript_runtime::json_at_optional_bool",
    ),
    runtime_intrinsic(
        "Json",
        "at_optional_int",
        "rsscript_runtime::json_at_optional_int",
    ),
    runtime_intrinsic(
        "Json",
        "at_optional_string",
        "rsscript_runtime::json_at_optional_string",
    ),
    runtime_intrinsic("Json", "at_string", "rsscript_runtime::json_at_string"),
    runtime_intrinsic(
        "Json",
        "at_string_or",
        "rsscript_runtime::json_at_string_or",
    ),
    runtime_intrinsic(
        "Json",
        "at_to_string",
        "rsscript_runtime::json_at_to_string",
    ),
    runtime_intrinsic(
        "Json",
        "at_to_string_or",
        "rsscript_runtime::json_at_to_string_or",
    ),
    runtime_intrinsic("Json", "at_or", "rsscript_runtime::json_at_or"),
    runtime_intrinsic("Json", "bool_at", "rsscript_runtime::json_bool_at"),
    runtime_intrinsic("Json", "bool_at_or", "rsscript_runtime::json_bool_at_or"),
    runtime_intrinsic(
        "Json",
        "json_bool_at_or",
        "rsscript_runtime::json_bool_at_or",
    ),
    runtime_intrinsic("Json", "clone", "rsscript_runtime::json_clone"),
    runtime_intrinsic("Json", "field", "rsscript_runtime::json_field"),
    runtime_intrinsic("Json", "field_bool", "rsscript_runtime::json_field_bool"),
    runtime_intrinsic("Json", "field_int", "rsscript_runtime::json_field_int"),
    runtime_intrinsic(
        "Json",
        "field_optional",
        "rsscript_runtime::json_field_optional",
    ),
    runtime_intrinsic(
        "Json",
        "field_optional_bool",
        "rsscript_runtime::json_field_optional_bool",
    ),
    runtime_intrinsic(
        "Json",
        "field_optional_int",
        "rsscript_runtime::json_field_optional_int",
    ),
    runtime_intrinsic(
        "Json",
        "field_optional_string",
        "rsscript_runtime::json_field_optional_string",
    ),
    runtime_intrinsic("Json", "value_at", "rsscript_runtime::json_value_at"),
    runtime_intrinsic("Json", "value", "rsscript_runtime::json_value"),
    runtime_intrinsic("Json", "values", "rsscript_runtime::json_values"),
    runtime_intrinsic("Json", "int_at", "rsscript_runtime::json_int_at"),
    runtime_intrinsic("Json", "int_at_or", "rsscript_runtime::json_int_at_or"),
    runtime_intrinsic("Json", "json_int_at_or", "rsscript_runtime::json_int_at_or"),
    runtime_intrinsic(
        "Json",
        "string_array",
        "rsscript_runtime::json_string_array",
    ),
    runtime_intrinsic("Json", "strings", "rsscript_runtime::json_strings"),
    runtime_intrinsic("Json", "string_at", "rsscript_runtime::json_string_at"),
    runtime_intrinsic(
        "Json",
        "string_at_or",
        "rsscript_runtime::json_string_at_or",
    ),
    runtime_intrinsic(
        "Json",
        "json_string_at_or",
        "rsscript_runtime::json_string_at_or",
    ),
    runtime_intrinsic("Json", "bool_field", "rsscript_runtime::json_bool_field"),
    runtime_intrinsic("Json", "int_field", "rsscript_runtime::json_int_field"),
    runtime_intrinsic("Json", "raw_field", "rsscript_runtime::json_raw_field"),
    runtime_intrinsic(
        "Json",
        "string_field",
        "rsscript_runtime::json_string_field",
    ),
    runtime_intrinsic(
        "Json",
        "field_string",
        "rsscript_runtime::json_field_string",
    ),
    runtime_intrinsic("Json", "is_array", "rsscript_runtime::json_is_array"),
    runtime_intrinsic("Json", "is_null", "rsscript_runtime::json_is_null"),
    runtime_intrinsic("Json", "is_object", "rsscript_runtime::json_is_object"),
    runtime_intrinsic("Json", "kind", "rsscript_runtime::json_kind"),
    runtime_intrinsic("Json", "object", "rsscript_runtime::json_object"),
    runtime_intrinsic("Json", "object_keys", "rsscript_runtime::json_object_keys"),
    runtime_intrinsic("Json", "object_len", "rsscript_runtime::json_object_len"),
    runtime_intrinsic("Json", "parse", "rsscript_runtime::json_parse"),
    runtime_intrinsic("Json", "json_parse", "rsscript_runtime::json_parse"),
    runtime_intrinsic("Json", "parse_file", "rsscript_runtime::json_parse_file"),
    runtime_intrinsic("Json", "to_string", "rsscript_runtime::json_to_string"),
    runtime_intrinsic(
        "Json",
        "to_string_at",
        "rsscript_runtime::json_to_string_at",
    ),
    runtime_intrinsic(
        "Json",
        "to_string_at_or",
        "rsscript_runtime::json_to_string_at_or",
    ),
    runtime_intrinsic(
        "Json",
        "quote_string",
        "rsscript_runtime::json_quote_string",
    ),
    runtime_intrinsic(
        "JsonError",
        "message",
        "rsscript_runtime::json_error_message",
    ),
    runtime_intrinsic("Deque", "clear", "rsscript_runtime::deque_clear"),
    runtime_intrinsic("Deque", "is_empty", "rsscript_runtime::deque_is_empty"),
    runtime_intrinsic("Deque", "len", "rsscript_runtime::deque_len"),
    runtime_intrinsic("Deque", "new", "rsscript_runtime::deque_new"),
    runtime_intrinsic("Deque", "pop_back", "rsscript_runtime::deque_pop_back"),
    runtime_intrinsic("Deque", "pop_front", "rsscript_runtime::deque_pop_front"),
    runtime_intrinsic("Deque", "push_back", "rsscript_runtime::deque_push_back"),
    runtime_intrinsic("Deque", "push_front", "rsscript_runtime::deque_push_front"),
    runtime_intrinsic("Deque", "to_list", "rsscript_runtime::deque_to_list"),
    runtime_intrinsic("Clone", "clone", "rsscript_runtime::clone_value"),
    runtime_intrinsic("Ord", "compare", "rsscript_runtime::ord_compare"),
    runtime_intrinsic("List", "all", "rsscript_runtime::list_all"),
    runtime_intrinsic("List", "any", "rsscript_runtime::list_any"),
    runtime_intrinsic("List", "append", "rsscript_runtime::list_append"),
    runtime_intrinsic("List", "clear", "rsscript_runtime::list_clear"),
    runtime_intrinsic("List", "consume", "rsscript_runtime::list_consume"),
    runtime_intrinsic("List", "contains", "rsscript_runtime::list_contains"),
    runtime_intrinsic(
        "List",
        "contains_value",
        "rsscript_runtime::list_contains_value",
    ),
    runtime_intrinsic("List", "count_where", "rsscript_runtime::list_count_where"),
    runtime_intrinsic("List", "dedup", "rsscript_runtime::list_dedup"),
    runtime_intrinsic("List", "enumerate", "rsscript_runtime::list_enumerate"),
    runtime_intrinsic("List", "filter", "rsscript_runtime::list_filter"),
    runtime_intrinsic("List", "find", "rsscript_runtime::list_find"),
    runtime_intrinsic("List", "first", "rsscript_runtime::list_first"),
    runtime_intrinsic("List", "flat_map", "rsscript_runtime::list_flat_map"),
    runtime_intrinsic("List", "flatten", "rsscript_runtime::list_flatten"),
    runtime_intrinsic("List", "fold", "rsscript_runtime::list_fold"),
    runtime_intrinsic("List", "get", "rsscript_runtime::list_get"),
    runtime_intrinsic("List", "group_by", "rsscript_runtime::list_group_by"),
    runtime_intrinsic("List", "is_empty", "rsscript_runtime::list_is_empty"),
    runtime_intrinsic("List", "join", "rsscript_runtime::list_join"),
    runtime_intrinsic("List", "last", "rsscript_runtime::list_last"),
    runtime_intrinsic("List", "len", "rsscript_runtime::list_len"),
    runtime_intrinsic("List", "map", "rsscript_runtime::list_map"),
    runtime_intrinsic("List", "max", "rsscript_runtime::list_max"),
    runtime_intrinsic("List", "min", "rsscript_runtime::list_min"),
    runtime_intrinsic("List", "new", "rsscript_runtime::list_new"),
    runtime_intrinsic("List", "pipeline", "rsscript_runtime::pipeline_from_list"),
    runtime_intrinsic("List", "partition", "rsscript_runtime::list_partition"),
    runtime_intrinsic("List", "pop", "rsscript_runtime::list_pop"),
    runtime_intrinsic("List", "push", "rsscript_runtime::list_push"),
    runtime_intrinsic("List", "reverse", "rsscript_runtime::list_reverse"),
    runtime_intrinsic("List", "remove_at", "rsscript_runtime::list_remove_at"),
    runtime_intrinsic("List", "set", "rsscript_runtime::list_set"),
    runtime_intrinsic("List", "skip", "rsscript_runtime::list_skip"),
    runtime_intrinsic("List", "slice", "rsscript_runtime::list_slice"),
    runtime_intrinsic("List", "sort", "rsscript_runtime::list_sort"),
    runtime_intrinsic("List", "sort_with", "rsscript_runtime::list_sort_with"),
    runtime_intrinsic("List", "sort_by", "rsscript_runtime::list_sort_by"),
    runtime_intrinsic("List", "sum", "rsscript_runtime::list_sum"),
    runtime_intrinsic("List", "take", "rsscript_runtime::list_take"),
    runtime_intrinsic("List", "to_json_strings", "rsscript_runtime::json_strings"),
    runtime_intrinsic("List", "to_json_values", "rsscript_runtime::json_values"),
    runtime_intrinsic("List", "try_fold", "rsscript_runtime::list_try_fold"),
    runtime_intrinsic("List", "zip", "rsscript_runtime::list_zip"),
    runtime_intrinsic("Log", "error", "rsscript_runtime::log_error"),
    runtime_intrinsic("Log", "error_json", "rsscript_runtime::log_error_json"),
    runtime_intrinsic("Log", "trace", "rsscript_runtime::log_trace"),
    runtime_intrinsic("Log", "write", "rsscript_runtime::log_write"),
    runtime_intrinsic("Log", "write_json", "rsscript_runtime::log_write_json"),
    runtime_intrinsic("Map", "clear", "rsscript_runtime::map_clear"),
    runtime_intrinsic("Map", "contains_key", "rsscript_runtime::map_contains_key"),
    runtime_intrinsic("Map", "filter", "rsscript_runtime::map_filter"),
    runtime_intrinsic("Map", "fold", "rsscript_runtime::map_fold"),
    runtime_intrinsic("Map", "for_each", "rsscript_runtime::map_for_each"),
    runtime_intrinsic("Map", "get", "rsscript_runtime::map_get"),
    runtime_intrinsic(
        "Map",
        "get_or_default",
        "rsscript_runtime::map_get_or_default",
    ),
    runtime_intrinsic("Map", "insert", "rsscript_runtime::map_insert"),
    runtime_intrinsic("Map", "insert_old", "rsscript_runtime::map_insert_old"),
    runtime_intrinsic("Map", "is_empty", "rsscript_runtime::map_is_empty"),
    runtime_intrinsic("Map", "keys", "rsscript_runtime::map_keys"),
    runtime_intrinsic("Map", "len", "rsscript_runtime::map_len"),
    runtime_intrinsic("Map", "map_values", "rsscript_runtime::map_map_values"),
    runtime_intrinsic("Map", "merge", "rsscript_runtime::map_merge"),
    runtime_intrinsic("Map", "new", "rsscript_runtime::map_new"),
    runtime_intrinsic("Map", "remove", "rsscript_runtime::map_remove"),
    runtime_intrinsic("Map", "try_fold", "rsscript_runtime::map_try_fold"),
    runtime_intrinsic("Map", "values", "rsscript_runtime::map_values"),
    runtime_intrinsic("Pipeline", "map", "rsscript_runtime::pipeline_map"),
    runtime_intrinsic("Pipeline", "filter", "rsscript_runtime::pipeline_filter"),
    runtime_intrinsic("Pipeline", "each", "rsscript_runtime::pipeline_each"),
    runtime_intrinsic("Pipeline", "try_map", "rsscript_runtime::pipeline_try_map"),
    runtime_intrinsic("Pipeline", "collect", "rsscript_runtime::pipeline_collect"),
    runtime_intrinsic(
        "PersistentMap",
        "clear",
        "rsscript_runtime::persistent_map_clear",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "contains_key",
        "rsscript_runtime::persistent_map_contains_key",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "get",
        "rsscript_runtime::persistent_map_get",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "insert",
        "rsscript_runtime::persistent_map_insert",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "is_empty",
        "rsscript_runtime::persistent_map_is_empty",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "len",
        "rsscript_runtime::persistent_map_len",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "new",
        "rsscript_runtime::persistent_map_new",
    ),
    runtime_intrinsic(
        "PersistentMap",
        "remove",
        "rsscript_runtime::persistent_map_remove",
    ),
    runtime_intrinsic("Random", "bool", "rsscript_runtime::random_bool"),
    runtime_intrinsic("Random", "bytes", "rsscript_runtime::random_bytes"),
    runtime_intrinsic("Random", "float", "rsscript_runtime::random_float"),
    runtime_intrinsic("Random", "int", "rsscript_runtime::random_int"),
    runtime_intrinsic("Random", "string", "rsscript_runtime::random_string"),
    runtime_intrinsic(
        "FalliblePipeline",
        "map",
        "rsscript_runtime::fallible_pipeline_map",
    ),
    runtime_intrinsic(
        "FalliblePipeline",
        "filter",
        "rsscript_runtime::fallible_pipeline_filter",
    ),
    runtime_intrinsic(
        "FalliblePipeline",
        "each",
        "rsscript_runtime::fallible_pipeline_each",
    ),
    runtime_intrinsic(
        "FalliblePipeline",
        "try_map",
        "rsscript_runtime::fallible_pipeline_try_map",
    ),
    runtime_intrinsic(
        "FalliblePipeline",
        "collect",
        "rsscript_runtime::fallible_pipeline_collect",
    ),
    runtime_intrinsic("Args", "all", "rsscript_runtime::args_all"),
    runtime_intrinsic("Args", "count", "rsscript_runtime::args_count"),
    runtime_intrinsic("Args", "get", "rsscript_runtime::args_get"),
    runtime_intrinsic(
        "Args",
        "get_or_default",
        "rsscript_runtime::args_get_or_default",
    ),
    runtime_intrinsic("Option", "and_then", "rsscript_runtime::option_and_then"),
    runtime_intrinsic("Option", "is_none", "rsscript_runtime::option_is_none"),
    runtime_intrinsic("Option", "is_some", "rsscript_runtime::option_is_some"),
    runtime_intrinsic("Option", "map", "rsscript_runtime::option_map"),
    runtime_intrinsic("Option", "ok_or", "rsscript_runtime::option_ok_or"),
    runtime_intrinsic("Option", "or", "rsscript_runtime::option_or"),
    runtime_intrinsic("Option", "filter", "rsscript_runtime::option_filter"),
    runtime_intrinsic("Option", "unwrap_or", "rsscript_runtime::option_unwrap_or"),
    runtime_intrinsic(
        "Option",
        "unwrap_or_else",
        "rsscript_runtime::option_unwrap_or_else",
    ),
    runtime_intrinsic("OS", "close", "rsscript_runtime::os_close"),
    runtime_intrinsic("Path", "extension", "rsscript_runtime::path_extension"),
    runtime_intrinsic("Path", "exists", "rsscript_runtime::directory_exists"),
    runtime_intrinsic("Path", "file_name", "rsscript_runtime::path_file_name"),
    runtime_intrinsic("Path", "from_string", "rsscript_runtime::path_from_string"),
    runtime_intrinsic("Path", "is_absolute", "rsscript_runtime::path_is_absolute"),
    runtime_intrinsic("Path", "is_dir", "rsscript_runtime::directory_is_dir"),
    runtime_intrinsic("Path", "is_file", "rsscript_runtime::directory_is_file"),
    runtime_intrinsic("Path", "join", "rsscript_runtime::path_join"),
    runtime_intrinsic(
        "Path",
        "list_files",
        "rsscript_runtime::directory_list_files",
    ),
    runtime_intrinsic(
        "Path",
        "list_paths",
        "rsscript_runtime::directory_list_paths",
    ),
    runtime_intrinsic("Path", "normalize", "rsscript_runtime::path_normalize"),
    runtime_intrinsic("Path", "parent", "rsscript_runtime::path_parent"),
    runtime_intrinsic("Path", "read_string", "rsscript_runtime::file_read_string"),
    runtime_intrinsic(
        "Path",
        "resolve_relative",
        "rsscript_runtime::path_resolve_relative",
    ),
    runtime_intrinsic(
        "Path",
        "safe_relative",
        "rsscript_runtime::path_safe_relative",
    ),
    runtime_intrinsic("Path", "starts_with", "rsscript_runtime::path_starts_with"),
    runtime_intrinsic("Path", "to_string", "rsscript_runtime::path_to_string"),
    runtime_intrinsic(
        "Path",
        "with_extension",
        "rsscript_runtime::path_with_extension",
    ),
    runtime_intrinsic(
        "Path",
        "write_string",
        "rsscript_runtime::file_write_string_to_path",
    ),
    runtime_intrinsic("Patch", "apply_text", "rsscript_runtime::patch_apply_text"),
    runtime_intrinsic("Process", "run", "rsscript_runtime::process_run"),
    runtime_intrinsic(
        "Process",
        "run_async",
        "rsscript_runtime::process_run_async",
    ),
    runtime_intrinsic(
        "Process",
        "run_timeout",
        "rsscript_runtime::process_run_timeout",
    ),
    runtime_intrinsic(
        "Process",
        "run_timeout_async",
        "rsscript_runtime::process_run_timeout_async",
    ),
    runtime_intrinsic(
        "Process",
        "run_request",
        "rsscript_runtime::process_run_request",
    ),
    runtime_intrinsic(
        "Process",
        "run_request_async",
        "rsscript_runtime::process_run_request_async",
    ),
    runtime_intrinsic(
        "Process",
        "run_request_cancellable_async",
        "rsscript_runtime::process_run_request_cancellable_async",
    ),
    runtime_intrinsic("Process", "stream", "rsscript_runtime::process_stream"),
    runtime_intrinsic(
        "Process",
        "run_stdout",
        "rsscript_runtime::process_run_stdout",
    ),
    runtime_intrinsic(
        "Process",
        "run_stdout_async",
        "rsscript_runtime::process_run_stdout_async",
    ),
    runtime_intrinsic(
        "Process",
        "run_stdout_timeout",
        "rsscript_runtime::process_run_stdout_timeout",
    ),
    runtime_intrinsic(
        "Process",
        "run_stdout_timeout_async",
        "rsscript_runtime::process_run_stdout_timeout_async",
    ),
    runtime_intrinsic(
        "Process",
        "run_many_stdout",
        "rsscript_runtime::process_run_many_stdout",
    ),
    runtime_intrinsic(
        "Process",
        "run_many_stdout_async",
        "rsscript_runtime::process_run_many_stdout_async",
    ),
    runtime_intrinsic(
        "Process",
        "run_many_stdout_timeout",
        "rsscript_runtime::process_run_many_stdout_timeout",
    ),
    runtime_intrinsic(
        "Process",
        "run_many_stdout_timeout_async",
        "rsscript_runtime::process_run_many_stdout_timeout_async",
    ),
    runtime_intrinsic("Regex", "captures", "rsscript_runtime::regex_captures"),
    runtime_intrinsic("Regex", "compile", "rsscript_runtime::regex_compile"),
    runtime_intrinsic("Regex", "find", "rsscript_runtime::regex_find"),
    runtime_intrinsic("Regex", "is_match", "rsscript_runtime::regex_is_match"),
    runtime_intrinsic(
        "Regex",
        "replace_all",
        "rsscript_runtime::regex_replace_all",
    ),
    runtime_intrinsic("Regex", "split", "rsscript_runtime::regex_split"),
    runtime_intrinsic(
        "RegexError",
        "message",
        "rsscript_runtime::regex_error_message",
    ),
    runtime_intrinsic("Request", "new", "rsscript_runtime::request_new"),
    runtime_intrinsic("Request", "path", "rsscript_runtime::request_path"),
    runtime_intrinsic("Response", "body", "rsscript_runtime::response_body"),
    runtime_intrinsic("Response", "ok", "rsscript_runtime::response_ok"),
    runtime_intrinsic("Response", "status", "rsscript_runtime::response_status"),
    runtime_intrinsic("Result", "and_then", "rsscript_runtime::result_and_then"),
    runtime_intrinsic("Result", "err", "rsscript_runtime::result_err"),
    runtime_intrinsic(
        "Result",
        "err_message",
        "rsscript_runtime::result_err_message",
    ),
    runtime_intrinsic("Result", "is_err", "rsscript_runtime::result_is_err"),
    runtime_intrinsic("Result", "is_ok", "rsscript_runtime::result_is_ok"),
    runtime_intrinsic("Result", "map", "rsscript_runtime::result_map"),
    runtime_intrinsic("Result", "map_error", "rsscript_runtime::result_map_error"),
    runtime_intrinsic("Result", "ok", "rsscript_runtime::result_ok"),
    runtime_intrinsic("Result", "unwrap_or", "rsscript_runtime::result_unwrap_or"),
    runtime_intrinsic(
        "Result",
        "unwrap_or_else",
        "rsscript_runtime::result_unwrap_or_else",
    ),
    runtime_intrinsic("Row", "field_string", "rsscript_runtime::row_field_string"),
    runtime_intrinsic("RowBuffer", "new", "rsscript_runtime::row_buffer_new"),
    runtime_intrinsic(
        "RuleLoader",
        "load_rules",
        "rsscript_runtime::rule_loader_load_rules",
    ),
    runtime_intrinsic("Set", "clear", "rsscript_runtime::set_clear"),
    runtime_intrinsic("Set", "contains", "rsscript_runtime::set_contains"),
    runtime_intrinsic("Set", "for_each", "rsscript_runtime::set_for_each"),
    runtime_intrinsic("Set", "union", "rsscript_runtime::set_union"),
    runtime_intrinsic("Set", "intersection", "rsscript_runtime::set_intersection"),
    runtime_intrinsic("Set", "difference", "rsscript_runtime::set_difference"),
    runtime_intrinsic("Set", "is_subset", "rsscript_runtime::set_is_subset"),
    runtime_intrinsic("Set", "insert", "rsscript_runtime::set_insert"),
    runtime_intrinsic("Set", "is_empty", "rsscript_runtime::set_is_empty"),
    runtime_intrinsic("Set", "len", "rsscript_runtime::set_len"),
    runtime_intrinsic("Set", "new", "rsscript_runtime::set_new"),
    runtime_intrinsic("Set", "remove", "rsscript_runtime::set_remove"),
    runtime_intrinsic("Set", "to_list", "rsscript_runtime::set_to_list"),
    runtime_intrinsic("SortedMap", "clear", "rsscript_runtime::sorted_map_clear"),
    runtime_intrinsic(
        "SortedMap",
        "contains_key",
        "rsscript_runtime::sorted_map_contains_key",
    ),
    runtime_intrinsic("SortedMap", "get", "rsscript_runtime::sorted_map_get"),
    runtime_intrinsic("SortedMap", "insert", "rsscript_runtime::sorted_map_insert"),
    runtime_intrinsic(
        "SortedMap",
        "is_empty",
        "rsscript_runtime::sorted_map_is_empty",
    ),
    runtime_intrinsic("SortedMap", "keys", "rsscript_runtime::sorted_map_keys"),
    runtime_intrinsic("SortedMap", "len", "rsscript_runtime::sorted_map_len"),
    runtime_intrinsic("SortedMap", "new", "rsscript_runtime::sorted_map_new"),
    runtime_intrinsic("SortedMap", "remove", "rsscript_runtime::sorted_map_remove"),
    runtime_intrinsic("SortedMap", "values", "rsscript_runtime::sorted_map_values"),
    runtime_intrinsic("SortedSet", "clear", "rsscript_runtime::sorted_set_clear"),
    runtime_intrinsic(
        "SortedSet",
        "contains",
        "rsscript_runtime::sorted_set_contains",
    ),
    runtime_intrinsic(
        "SortedSet",
        "is_empty",
        "rsscript_runtime::sorted_set_is_empty",
    ),
    runtime_intrinsic("SortedSet", "insert", "rsscript_runtime::sorted_set_insert"),
    runtime_intrinsic("SortedSet", "len", "rsscript_runtime::sorted_set_len"),
    runtime_intrinsic("SortedSet", "new", "rsscript_runtime::sorted_set_new"),
    runtime_intrinsic("SortedSet", "remove", "rsscript_runtime::sorted_set_remove"),
    runtime_intrinsic(
        "SortedSet",
        "to_list",
        "rsscript_runtime::sorted_set_to_list",
    ),
    runtime_intrinsic("Char", "compare", "rsscript_runtime::char_compare"),
    runtime_intrinsic("Char", "from_code", "rsscript_runtime::char_from_code"),
    runtime_intrinsic(
        "Char",
        "is_alphanumeric",
        "rsscript_runtime::char_is_alphanumeric",
    ),
    runtime_intrinsic("Char", "is_alpha", "rsscript_runtime::char_is_alpha"),
    runtime_intrinsic("Char", "is_digit", "rsscript_runtime::char_is_digit"),
    runtime_intrinsic("Char", "is_lower", "rsscript_runtime::char_is_lower"),
    runtime_intrinsic("Char", "is_upper", "rsscript_runtime::char_is_upper"),
    runtime_intrinsic(
        "Char",
        "is_whitespace",
        "rsscript_runtime::char_is_whitespace",
    ),
    runtime_intrinsic("Char", "to_code", "rsscript_runtime::char_to_code"),
    runtime_intrinsic("Char", "to_lower", "rsscript_runtime::char_to_lower"),
    runtime_intrinsic("Char", "to_string", "rsscript_runtime::char_to_string"),
    runtime_intrinsic("Char", "to_upper", "rsscript_runtime::char_to_upper"),
    runtime_intrinsic("Int", "bit_and", "rsscript_runtime::int_bit_and"),
    runtime_intrinsic("Int", "bit_not", "rsscript_runtime::int_bit_not"),
    runtime_intrinsic("Int", "bit_or", "rsscript_runtime::int_bit_or"),
    runtime_intrinsic("Int", "bit_xor", "rsscript_runtime::int_bit_xor"),
    runtime_intrinsic("Int", "shift_left", "rsscript_runtime::int_shift_left"),
    runtime_intrinsic("Int", "shift_right", "rsscript_runtime::int_shift_right"),
    runtime_intrinsic("Int", "to_string", "rsscript_runtime::string_from_int"),
    runtime_intrinsic("Int", "to_float", "rsscript_runtime::int_to_float"),
    runtime_intrinsic("Float", "to_string", "rsscript_runtime::float_to_string"),
    runtime_intrinsic("Float", "is_finite", "rsscript_runtime::float_is_finite"),
    runtime_intrinsic(
        "Float",
        "is_infinite",
        "rsscript_runtime::float_is_infinite",
    ),
    runtime_intrinsic("Float", "is_nan", "rsscript_runtime::float_is_nan"),
    runtime_intrinsic("Math", "abs", "rsscript_runtime::math_abs"),
    runtime_intrinsic("Math", "abs_float", "rsscript_runtime::math_abs_float"),
    runtime_intrinsic("Math", "ceil", "rsscript_runtime::math_ceil"),
    runtime_intrinsic("Math", "clamp", "rsscript_runtime::math_clamp"),
    runtime_intrinsic("Math", "clamp_float", "rsscript_runtime::math_clamp_float"),
    runtime_intrinsic("Math", "cos", "rsscript_runtime::math_cos"),
    runtime_intrinsic("Math", "exp", "rsscript_runtime::math_exp"),
    runtime_intrinsic("Math", "exp2", "rsscript_runtime::math_exp2"),
    runtime_intrinsic("Math", "floor", "rsscript_runtime::math_floor"),
    runtime_intrinsic("Math", "log", "rsscript_runtime::math_log"),
    runtime_intrinsic("Math", "log2", "rsscript_runtime::math_log2"),
    runtime_intrinsic("Math", "max", "rsscript_runtime::math_max"),
    runtime_intrinsic("Math", "max_float", "rsscript_runtime::math_max_float"),
    runtime_intrinsic("Math", "min", "rsscript_runtime::math_min"),
    runtime_intrinsic("Math", "min_float", "rsscript_runtime::math_min_float"),
    runtime_intrinsic("Math", "pow", "rsscript_runtime::math_pow"),
    runtime_intrinsic("Math", "pow_float", "rsscript_runtime::math_pow_float"),
    runtime_intrinsic("Math", "round", "rsscript_runtime::math_round"),
    runtime_intrinsic("Math", "sin", "rsscript_runtime::math_sin"),
    runtime_intrinsic("Math", "sqrt", "rsscript_runtime::math_sqrt"),
    runtime_intrinsic("Math", "tanh", "rsscript_runtime::math_tanh"),
    runtime_intrinsic("Math", "trunc_float", "rsscript_runtime::math_trunc_float"),
    runtime_intrinsic("String", "after", "rsscript_runtime::string_after"),
    runtime_intrinsic("String", "char_at", "rsscript_runtime::string_char_at"),
    runtime_intrinsic("String", "contains", "rsscript_runtime::string_contains"),
    runtime_intrinsic("String", "count", "rsscript_runtime::string_count"),
    runtime_intrinsic("String", "before", "rsscript_runtime::string_before"),
    runtime_intrinsic("String", "concat", "rsscript_runtime::string_concat"),
    runtime_intrinsic("String", "copy", "rsscript_runtime::string_copy"),
    runtime_intrinsic("String", "clone", "rsscript_runtime::string_copy"),
    runtime_intrinsic("String", "ends_with", "rsscript_runtime::string_ends_with"),
    runtime_intrinsic("String", "env", "rsscript_runtime::env_get"),
    runtime_intrinsic("String", "env_or", "rsscript_runtime::env_get_or_default"),
    runtime_intrinsic("String", "format", "rsscript_runtime::string_format"),
    runtime_intrinsic("String", "from_bool", "rsscript_runtime::string_from_bool"),
    runtime_intrinsic(
        "String",
        "from_float",
        "rsscript_runtime::string_from_float",
    ),
    runtime_intrinsic("String", "from_int", "rsscript_runtime::string_from_int"),
    runtime_intrinsic("String", "is_empty", "rsscript_runtime::string_is_empty"),
    runtime_intrinsic("String", "index_of", "rsscript_runtime::string_index_of"),
    runtime_intrinsic("String", "join", "rsscript_runtime::string_join"),
    runtime_intrinsic("String", "chars", "rsscript_runtime::string_chars"),
    runtime_intrinsic("String", "len", "rsscript_runtime::string_len"),
    runtime_intrinsic("String", "lines", "rsscript_runtime::string_lines"),
    runtime_intrinsic("String", "pad_left", "rsscript_runtime::string_pad_left"),
    runtime_intrinsic("String", "pad_right", "rsscript_runtime::string_pad_right"),
    runtime_intrinsic("String", "replace", "rsscript_runtime::string_replace"),
    runtime_intrinsic(
        "String",
        "replace_first",
        "rsscript_runtime::string_replace_first",
    ),
    runtime_intrinsic("String", "repeat", "rsscript_runtime::string_repeat"),
    runtime_intrinsic("String", "parse_int", "rsscript_runtime::string_parse_int"),
    runtime_intrinsic(
        "String",
        "parse_float",
        "rsscript_runtime::string_parse_float",
    ),
    runtime_intrinsic("String", "reverse", "rsscript_runtime::string_reverse"),
    runtime_intrinsic(
        "String",
        "safe_relative",
        "rsscript_runtime::path_safe_relative",
    ),
    runtime_intrinsic("String", "slice", "rsscript_runtime::string_slice"),
    runtime_intrinsic(
        "String",
        "strip_prefix",
        "rsscript_runtime::string_strip_prefix",
    ),
    runtime_intrinsic(
        "String",
        "starts_with",
        "rsscript_runtime::string_starts_with",
    ),
    runtime_intrinsic("String", "split", "rsscript_runtime::string_split"),
    runtime_intrinsic(
        "String",
        "to_lowercase",
        "rsscript_runtime::string_to_lowercase",
    ),
    runtime_intrinsic("String", "to_path", "rsscript_runtime::path_from_string"),
    runtime_intrinsic("String", "to_url", "rsscript_runtime::url_from_string"),
    runtime_intrinsic("String", "to_bytes", "rsscript_runtime::bytes_from_string"),
    runtime_intrinsic(
        "String",
        "to_uppercase",
        "rsscript_runtime::string_to_uppercase",
    ),
    runtime_intrinsic("String", "trim", "rsscript_runtime::string_trim"),
    runtime_intrinsic("String", "trim_end", "rsscript_runtime::string_trim_end"),
    runtime_intrinsic(
        "String",
        "trim_start",
        "rsscript_runtime::string_trim_start",
    ),
    runtime_intrinsic("String", "view", "rsscript_runtime::string_view"),
    runtime_intrinsic("StringView", "after", "rsscript_runtime::string_view_after"),
    runtime_intrinsic(
        "StringView",
        "before",
        "rsscript_runtime::string_view_before",
    ),
    runtime_intrinsic(
        "StringView",
        "contains",
        "rsscript_runtime::string_view_contains",
    ),
    runtime_intrinsic(
        "StringView",
        "is_empty",
        "rsscript_runtime::string_view_is_empty",
    ),
    runtime_intrinsic("StringView", "len", "rsscript_runtime::string_view_len"),
    runtime_intrinsic("StringView", "slice", "rsscript_runtime::string_view_slice"),
    runtime_intrinsic(
        "StringView",
        "starts_with",
        "rsscript_runtime::string_view_starts_with",
    ),
    runtime_intrinsic(
        "StringView",
        "to_string",
        "rsscript_runtime::string_view_to_string",
    ),
    runtime_intrinsic(
        "StringBuilder",
        "finish",
        "rsscript_runtime::string_builder_finish",
    ),
    runtime_intrinsic(
        "StringBuilder",
        "new",
        "rsscript_runtime::string_builder_new",
    ),
    runtime_intrinsic(
        "StringBuilder",
        "push",
        "rsscript_runtime::string_builder_push",
    ),
    runtime_intrinsic("TempDir", "keep", "rsscript_runtime::tempdir_keep"),
    runtime_intrinsic("TempDir", "new", "rsscript_runtime::tempdir_new"),
    runtime_intrinsic("TempDir", "new_in", "rsscript_runtime::tempdir_new_in"),
    runtime_intrinsic("TempDir", "path", "rsscript_runtime::tempdir_path"),
    runtime_intrinsic("Toml", "parse_file", "rsscript_runtime::toml_parse_file"),
    runtime_intrinsic(
        "Timer",
        "sleep",
        "rsscript_runtime::timer_sleep_native_start",
    ),
    runtime_intrinsic(
        "Timer",
        "sleep_until",
        "rsscript_runtime::timer_sleep_until_native_start",
    ),
    runtime_intrinsic(
        "Timer",
        "sleep_cancellable",
        "rsscript_runtime::timer_sleep_cancellable_native_start",
    ),
    runtime_intrinsic("Url", "from_string", "rsscript_runtime::url_from_string"),
    runtime_intrinsic(
        "Url",
        "decode_component",
        "rsscript_runtime::url_decode_component",
    ),
    runtime_intrinsic(
        "Url",
        "encode_component",
        "rsscript_runtime::url_encode_component",
    ),
    runtime_intrinsic("Url", "to_string", "rsscript_runtime::string_copy"),
    runtime_intrinsic("Uuid", "new_v4", "rsscript_runtime::uuid_new_v4"),
    runtime_intrinsic(
        "Workspace",
        "resolve",
        "rsscript_runtime::path_resolve_relative",
    ),
    runtime_intrinsic("Yaml", "parse", "rsscript_runtime::yaml_parse"),
    runtime_intrinsic("Yaml", "parse_file", "rsscript_runtime::yaml_parse_file"),
];

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::Path;

    use crate::interfaces::default_interfaces;
    use crate::syntax::ast::Item;
    use crate::syntax::parse_source;

    use super::RUNTIME_INTRINSICS;

    #[test]
    fn runtime_intrinsic_keys_are_unique() {
        let mut seen = HashSet::new();
        for intrinsic in RUNTIME_INTRINSICS {
            assert!(
                seen.insert((intrinsic.namespace, intrinsic.name)),
                "duplicate runtime intrinsic {}.{}",
                intrinsic.namespace,
                intrinsic.name
            );
        }
    }

    #[test]
    fn runtime_intrinsics_have_core_interface_signatures() {
        let public_functions = core_public_function_params();

        for intrinsic in RUNTIME_INTRINSICS {
            let signature = format!("{}.{}", intrinsic.namespace, intrinsic.name);
            assert!(
                public_functions.contains_key(&signature),
                "runtime intrinsic `{signature}` has no bundled public core interface signature"
            );
        }
    }

    #[test]
    fn core_public_native_functions_have_runtime_intrinsics() {
        let native_functions = core_public_native_function_signatures();
        let runtime_functions = runtime_intrinsic_signatures();
        let missing = native_functions
            .difference(&runtime_functions)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "core public native functions missing runtime ABI entries: {missing:?}"
        );
    }

    #[test]
    fn runtime_intrinsic_managed_handle_args_match_core_params() {
        let public_functions = core_public_function_params();
        for intrinsic in RUNTIME_INTRINSICS {
            if intrinsic.managed_handle_args.is_empty() {
                continue;
            }
            let signature = format!("{}.{}", intrinsic.namespace, intrinsic.name);
            let params = public_functions.get(&signature).unwrap_or_else(|| {
                panic!("runtime intrinsic `{signature}` should have core params")
            });
            for arg in intrinsic.managed_handle_args {
                assert!(
                    params.contains(*arg),
                    "runtime intrinsic `{signature}` declares managed handle arg `{arg}`, but core params are {params:?}"
                );
            }
        }
    }

    fn core_public_function_params() -> HashMap<String, HashSet<String>> {
        let mut public_functions = HashMap::new();
        for (path, source) in default_interfaces() {
            let program = parse_source(path, source);
            let protocol_names = program
                .protocols
                .iter()
                .map(|protocol| protocol.name.as_str())
                .collect::<HashSet<_>>();
            for item in program.items {
                if let Item::Function(function) = item
                    && (function.is_public
                        || function
                            .name
                            .rsplit_once('.')
                            .is_some_and(|(namespace, _)| protocol_names.contains(namespace)))
                {
                    public_functions.insert(
                        function.name,
                        function
                            .params
                            .into_iter()
                            .map(|param| param.name)
                            .collect(),
                    );
                }
            }
        }
        public_functions
    }

    fn core_public_native_function_signatures() -> HashSet<String> {
        let mut native_functions = HashSet::new();
        for (path, source) in default_interfaces() {
            let program = parse_source(path, source);
            let protocol_names = program
                .protocols
                .iter()
                .map(|protocol| protocol.name.as_str())
                .collect::<HashSet<_>>();
            for item in program.items {
                if let Item::Function(function) = item
                    && function.is_native
                    && (function.is_public
                        || function
                            .name
                            .rsplit_once('.')
                            .is_some_and(|(namespace, _)| protocol_names.contains(namespace)))
                {
                    native_functions.insert(function.name);
                }
            }
        }
        native_functions
    }

    fn runtime_intrinsic_signatures() -> HashSet<String> {
        RUNTIME_INTRINSICS
            .iter()
            .map(|intrinsic| format!("{}.{}", intrinsic.namespace, intrinsic.name))
            .collect()
    }

    #[test]
    fn runtime_intrinsic_rust_targets_exist() {
        let runtime_functions = runtime_public_functions();
        for intrinsic in RUNTIME_INTRINSICS {
            let target = intrinsic
                .rust_target
                .strip_prefix("rsscript_runtime::")
                .expect("runtime intrinsic target should be in rsscript_runtime");
            assert!(
                runtime_functions.contains(target),
                "runtime intrinsic {}.{} points to missing Rust runtime target `{}`",
                intrinsic.namespace,
                intrinsic.name,
                intrinsic.rust_target
            );
        }
    }

    fn runtime_public_functions() -> HashSet<String> {
        let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
        let mut functions = HashSet::new();
        for entry in fs::read_dir(&runtime_src).expect("runtime/src should be readable") {
            let path = entry.expect("runtime/src entry should be readable").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("runtime source should be readable");
            collect_public_functions(&source, &mut functions);
        }
        functions
    }

    fn collect_public_functions(source: &str, functions: &mut HashSet<String>) {
        let bytes = source.as_bytes();
        let mut index = 0;
        while let Some(relative) = source[index..].find("pub ") {
            index += relative + "pub ".len();
            let mut cursor = skip_ascii_whitespace(bytes, index);
            if source[cursor..].starts_with("async ") {
                cursor += "async ".len();
                cursor = skip_ascii_whitespace(bytes, cursor);
            }
            if !source[cursor..].starts_with("fn ") {
                continue;
            }
            cursor += "fn ".len();
            cursor = skip_ascii_whitespace(bytes, cursor);
            let start = cursor;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
            {
                cursor += 1;
            }
            if cursor > start {
                functions.insert(source[start..cursor].to_string());
            }
            index = cursor;
        }
    }

    fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        index
    }
}
