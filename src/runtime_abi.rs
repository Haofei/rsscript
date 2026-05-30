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

const RUNTIME_INTRINSICS: &[RuntimeIntrinsic] = &[
    runtime_intrinsic("Assert", "equal", "rsscript_runtime::assert_equal"),
    runtime_intrinsic(
        "Assert",
        "equal_bool",
        "rsscript_runtime::assert_equal_bool",
    ),
    runtime_intrinsic("Assert", "equal_int", "rsscript_runtime::assert_equal_int"),
    runtime_intrinsic("Buffer", "clear", "rsscript_runtime::buffer_clear"),
    runtime_intrinsic("Buffer", "consume", "rsscript_runtime::buffer_consume"),
    runtime_intrinsic("Buffer", "new", "rsscript_runtime::buffer_new"),
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
    runtime_intrinsic(
        "Directory",
        "list_files",
        "rsscript_runtime::directory_list_files",
    ),
    runtime_intrinsic("Csv", "open_read", "rsscript_runtime::csv_open_read"),
    runtime_intrinsic("Csv", "parse_row", "rsscript_runtime::csv_parse_row"),
    runtime_intrinsic("Csv", "read_into", "rsscript_runtime::csv_read_into"),
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
    runtime_intrinsic("File", "open", "rsscript_runtime::file_open"),
    runtime_intrinsic("File", "open_read", "rsscript_runtime::file_open_read"),
    runtime_intrinsic("File", "open_write", "rsscript_runtime::file_open_write"),
    runtime_intrinsic("File", "read_all", "rsscript_runtime::file_read_all"),
    runtime_intrinsic(
        "File",
        "read_all_string",
        "rsscript_runtime::file_read_all_string",
    ),
    runtime_intrinsic("File", "read_into", "rsscript_runtime::file_read_into"),
    runtime_intrinsic("File", "write", "rsscript_runtime::file_write"),
    runtime_intrinsic(
        "File",
        "write_buffer",
        "rsscript_runtime::file_write_buffer",
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
    runtime_intrinsic("ImageCache", "len", "rsscript_runtime::image_cache_len"),
    runtime_intrinsic("ImageCache", "new", "rsscript_runtime::image_cache_new"),
    runtime_intrinsic_with_handles(
        "ImageCache",
        "store",
        "rsscript_runtime::image_cache_store",
        &["image"],
    ),
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
    runtime_intrinsic("Json", "array_get", "rsscript_runtime::json_array_get"),
    runtime_intrinsic("Json", "array_len", "rsscript_runtime::json_array_len"),
    runtime_intrinsic("Json", "as_string", "rsscript_runtime::json_as_string"),
    runtime_intrinsic("Json", "field", "rsscript_runtime::json_field"),
    runtime_intrinsic("Json", "field_bool", "rsscript_runtime::json_field_bool"),
    runtime_intrinsic("Json", "field_int", "rsscript_runtime::json_field_int"),
    runtime_intrinsic(
        "Json",
        "field_string",
        "rsscript_runtime::json_field_string",
    ),
    runtime_intrinsic("Json", "parse", "rsscript_runtime::json_parse"),
    runtime_intrinsic("Json", "parse_file", "rsscript_runtime::json_parse_file"),
    runtime_intrinsic(
        "JsonError",
        "message",
        "rsscript_runtime::json_error_message",
    ),
    runtime_intrinsic("List", "consume", "rsscript_runtime::list_consume"),
    runtime_intrinsic("List", "get", "rsscript_runtime::list_get"),
    runtime_intrinsic("List", "len", "rsscript_runtime::list_len"),
    runtime_intrinsic("List", "new", "rsscript_runtime::list_new"),
    runtime_intrinsic("List", "push", "rsscript_runtime::list_push"),
    runtime_intrinsic("Log", "write", "rsscript_runtime::log_write"),
    runtime_intrinsic("Args", "count", "rsscript_runtime::args_count"),
    runtime_intrinsic(
        "Args",
        "get_or_default",
        "rsscript_runtime::args_get_or_default",
    ),
    runtime_intrinsic("OS", "close", "rsscript_runtime::os_close"),
    runtime_intrinsic("Path", "from_string", "rsscript_runtime::path_from_string"),
    runtime_intrinsic("Path", "join", "rsscript_runtime::path_join"),
    runtime_intrinsic("Request", "new", "rsscript_runtime::request_new"),
    runtime_intrinsic("Request", "path", "rsscript_runtime::request_path"),
    runtime_intrinsic("Response", "body", "rsscript_runtime::response_body"),
    runtime_intrinsic("Response", "ok", "rsscript_runtime::response_ok"),
    runtime_intrinsic("Response", "status", "rsscript_runtime::response_status"),
    runtime_intrinsic("Row", "field_string", "rsscript_runtime::row_field_string"),
    runtime_intrinsic("RowBuffer", "new", "rsscript_runtime::row_buffer_new"),
    runtime_intrinsic(
        "RuleLoader",
        "load_rules",
        "rsscript_runtime::rule_loader_load_rules",
    ),
    runtime_intrinsic("String", "contains", "rsscript_runtime::string_contains"),
    runtime_intrinsic("String", "before", "rsscript_runtime::string_before"),
    runtime_intrinsic("String", "ends_with", "rsscript_runtime::string_ends_with"),
    runtime_intrinsic("String", "from_bool", "rsscript_runtime::string_from_bool"),
    runtime_intrinsic("String", "from_int", "rsscript_runtime::string_from_int"),
    runtime_intrinsic("String", "len", "rsscript_runtime::string_len"),
    runtime_intrinsic("String", "lines", "rsscript_runtime::string_lines"),
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
    runtime_intrinsic("Toml", "parse_file", "rsscript_runtime::toml_parse_file"),
    runtime_intrinsic("Url", "from_string", "rsscript_runtime::url_from_string"),
];
