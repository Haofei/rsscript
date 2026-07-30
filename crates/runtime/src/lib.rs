#![forbid(unsafe_code)]

mod asserts;
mod async_runtime;
mod channel;
mod clock;
mod collections;
mod compatibility;
mod date;
mod diagnostics;
mod domain;
mod encoding;
mod env;
mod error;
mod fs;
mod hash;
mod json;
mod managed;
mod math;
#[cfg(feature = "net")]
mod network;
mod operation_context;
mod process;
mod random;
mod regex;
mod resource_budget;
mod resource_pool;
#[cfg(feature = "net")]
mod socket;
mod string_helpers;
mod tempdir;
mod text_edit;
#[cfg(feature = "net")]
mod websocket;

#[cfg(feature = "net")]
pub(crate) use async_runtime::cancellation_token_cancelled;
pub(crate) use clock::deadline_remaining_duration;

// Generated Rust uses root paths. Keep that compatibility surface explicit and
// share the same manifest with `abi` so the two cannot drift.
macro_rules! runtime_abi_exports {
    () => {
        pub use crate::asserts::{assert_equal, assert_equal_bool, assert_equal_int};
        pub use crate::async_runtime::{
            AsyncPoll, CancellationToken, Context, DeferredPending, Executor, LoopControl,
            LoopResultPending, NativeAsyncCompleter, NativeAsyncPending, Pending, PollFnPending,
            ReadyPending, RssCancellationSource, RssCancellationToken, RuntimeServices, TaskGroup,
            TaskGroupJoin, TaskGroupScope, ThenPending, TimerSleepPending, TryPending, WakeHandle,
            cancellation_never, cancellation_source_cancel, cancellation_source_new,
            cancellation_source_token, cancellation_token_is_cancelled, native_async_pending,
            pending_defer, pending_loop_result, pending_poll_fn, pending_ready, pending_then,
            pending_try, run_pending, spawn_tokio_native, spawn_tokio_native_with_cancellation,
            spawn_tokio_native_with_services, timer_sleep_cancellable_native_start,
            timer_sleep_native_start, timer_sleep_native_start_with_cancellation,
            timer_sleep_start, timer_sleep_until_native_start, tokio_native_runtime_worker_threads,
            trace_async_runtime_phase,
        };
        pub use crate::channel::{
            ChannelError, RecvPending, RssChannel, RssReceiver, RssSender, RssStream, SendPending,
            StreamNextPending, channel_bounded, channel_error_message, channel_receiver,
            channel_sender, receiver_close, receiver_into_stream, receiver_recv,
            receiver_recv_cancellable, sender_close, sender_send, sender_send_cancellable,
            stream_collect_list, stream_collect_list_with_limits, stream_from_external_receiver,
            stream_from_iterator, stream_from_list, stream_next,
        };
        pub use crate::clock::{
            RssDeadline, RssDuration, RssInstant, clock_now, clock_system_unix_ms, deadline_after,
            deadline_after_ms, deadline_is_expired, deadline_remaining_ms, duration_add,
            duration_as_ms, duration_as_seconds, duration_ms, duration_seconds, instant_elapsed,
        };
        pub use crate::collections::{
            RssFalliblePipeline, RssPersistentMap, RssPipeline, buffer_clear, buffer_consume,
            buffer_is_empty, buffer_len, buffer_new, buffer_view, buffer_view_is_empty,
            buffer_view_len, buffer_view_slice, buffer_view_to_bytes, bytes_concat, bytes_consume,
            bytes_from_buffer, bytes_from_string, bytes_from_uints, bytes_is_empty, bytes_len,
            bytes_slice, bytes_to_string, bytes_to_uints, bytes_view, bytes_view_is_empty,
            bytes_view_len, bytes_view_slice, bytes_view_starts_with, bytes_view_to_bytes,
            checked_list_index, clone_value, deque_clear, deque_is_empty, deque_len, deque_new,
            deque_pop_back, deque_pop_front, deque_push_back, deque_push_front, deque_to_list,
            fallible_pipeline_collect, fallible_pipeline_each, fallible_pipeline_filter,
            fallible_pipeline_map, fallible_pipeline_try_map, list_all, list_any, list_append,
            list_clear, list_consume, list_contains, list_contains_value, list_count_where,
            list_dedup, list_enumerate, list_filter, list_find, list_first, list_flat_map,
            list_flatten, list_fold, list_get, list_group_by, list_is_empty, list_join, list_last,
            list_len, list_map, list_max, list_min, list_new, list_partition, list_pop, list_push,
            list_remove_at, list_reverse, list_set, list_skip, list_slice, list_sort, list_sort_by,
            list_sort_with, list_sum, list_take, list_try_fold, list_zip, map_clear,
            map_contains_key, map_filter, map_fold, map_for_each, map_from_entries, map_get,
            map_get_or_default, map_insert, map_insert_old, map_is_empty, map_keys, map_len,
            map_map_values, map_merge, map_new, map_remove, map_try_fold, map_values,
            option_and_then, option_filter, option_is_none, option_is_some, option_map,
            option_ok_or, option_or, option_unwrap_or, option_unwrap_or_else, ord_compare,
            persistent_map_clear, persistent_map_contains_key, persistent_map_get,
            persistent_map_insert, persistent_map_is_empty, persistent_map_len, persistent_map_new,
            persistent_map_remove, pipeline_collect, pipeline_each, pipeline_filter,
            pipeline_from_list, pipeline_map, pipeline_try_map, result_and_then, result_err,
            result_err_message, result_is_err, result_is_ok, result_map, result_map_error,
            result_ok, result_unwrap_or, result_unwrap_or_else, set_clear, set_contains,
            set_difference, set_for_each, set_insert, set_intersection, set_is_empty,
            set_is_subset, set_len, set_new, set_remove, set_to_list, set_union, sorted_map_clear,
            sorted_map_contains_key, sorted_map_get, sorted_map_insert, sorted_map_is_empty,
            sorted_map_keys, sorted_map_len, sorted_map_new, sorted_map_remove, sorted_map_values,
            sorted_set_clear, sorted_set_contains, sorted_set_insert, sorted_set_is_empty,
            sorted_set_len, sorted_set_new, sorted_set_remove, sorted_set_to_list, url_from_string,
        };
        pub use crate::date::{
            date_add_days, date_add_ms, date_day, date_days_between, date_days_in_month,
            date_format_iso, date_format_ymd, date_hour, date_is_leap_year, date_minute,
            date_month, date_parse_iso, date_parse_ymd, date_second, date_start_of_day,
            date_weekday, date_year,
        };
        pub use crate::diagnostics::{
            ManagedValue, RUNTIME_DIAGNOSTIC_PREFIX, Resource,
            install_runtime_diagnostic_panic_hook,
        };
        pub use crate::domain::{
            Cache, Config, ConfigError, ConfigStore, ConfigValue, Counter, CsvError, DbConnection,
            DbError, Environment, FunctionObject, GlobalConfig, HttpError, HttpRequest, Image,
            ImageError, Request, Response, Row, RowBuffer, Rule, RuntimeEnvironmentHandle,
            RuntimeEnvironmentMut, RuntimeFunctionHandle, RuntimeImageRef, TimerError, cache_get,
            cache_insert, cache_lookup, cache_new, config_load, config_name, config_new,
            config_rule_count, config_store_name, config_store_new, config_store_replace,
            counter_add, counter_new, counter_value, csv_open_read, csv_parse_row, csv_read_into,
            csv_read_into_with_budget, csv_rows, db_close, db_connection_open, db_connection_query,
            db_connection_try_open, environment_bind_function, environment_child,
            environment_has_function, environment_has_parent, environment_root,
            function_object_has_closure, function_object_new, global_config_new,
            global_config_replace, global_config_rule_count, http_error_message,
            http_request_debug_summary, http_request_json, http_request_with_header,
            http_request_with_retry, http_request_with_timeout, http_response_bytes,
            http_response_is_success, http_response_lines, http_response_status,
            http_response_text, image_debug_summary, image_inspect, image_load, image_normalize,
            image_resize, image_save, image_sharpen, request_new, request_path, response_body,
            response_ok, response_status, row_buffer_new, row_field_string, rule_loader_load_rules,
        };
        #[cfg(feature = "net")]
        pub use crate::domain::{
            http_get, http_get_async, http_get_async_with_context, http_get_retry_async,
            http_get_timeout_async, http_post_form, http_post_form_async, http_post_json,
            http_post_json_async, http_post_json_bearer_retry_async, http_post_json_retry_async,
            http_post_json_timeout_async, http_send_async, http_send_async_with_context,
        };
        pub use crate::encoding::{
            DecodeError, base64_decode, base64_decode_string, base64_encode, base64_encode_bytes,
            decode_error_message, gzip_decompress_bytes, gzip_decompress_bytes_with_budget,
            hex_decode, hex_encode, hex_encode_string, url_decode_component, url_encode_component,
        };
        pub use crate::env::{
            env_current_dir, env_get, env_get_or_default, env_home_dir, env_run_workspace_root,
            env_set, env_set_current_dir, env_temp_dir,
        };
        pub use crate::error::{RuntimeError, RuntimeErrorKind, SourceSpan};
        #[allow(deprecated)]
        pub use crate::fs::{
            File, FileError, FileMetadata, RUNTIME_DIRECTORY_MAX_DEPTH,
            RUNTIME_DIRECTORY_MAX_ENTRIES, RUNTIME_DIRECTORY_MAX_PATH_BYTES,
            RUNTIME_READ_CEILING_BYTES, RuntimeBytes, RuntimePath, directory_copy_file,
            directory_create, directory_create_all, directory_exists, directory_is_dir,
            directory_is_file, directory_list_files, directory_list_paths, directory_metadata,
            directory_read_string, directory_remove_dir_all, directory_remove_file,
            directory_rename, directory_write_string, file_append_bytes, file_append_string,
            file_bytes_stream, file_error_message, file_exists, file_open, file_open_read,
            file_open_write, file_read_all, file_read_all_async, file_read_all_string,
            file_read_all_string_async, file_read_all_with_budget, file_read_bytes,
            file_read_bytes_from_offset, file_read_bytes_from_offset_with_budget,
            file_read_bytes_with_budget, file_read_into, file_read_into_with_budget,
            file_read_string, file_read_string_with_budget, file_remove, file_write,
            file_write_async, file_write_atomic, file_write_buffer, file_write_bytes,
            file_write_string, file_write_string_async, file_write_string_to_path, path_exists,
            path_extension, path_file_name, path_from_string, path_is_absolute, path_join,
            path_lexically_resolve_relative, path_lexically_safe_relative, path_normalize,
            path_parent, path_resolve_relative, path_safe_relative, path_starts_with,
            path_to_string, path_with_extension,
        };
        pub use crate::hash::{
            hash_sha3_224_bytes, hash_sha3_256_bytes, hash_sha256_bytes, hash_sha256_file,
            hash_sha256_string, hash_shake128_bytes, hmac_sha256_bytes, hmac_sha256_string,
        };
        pub use crate::json::{
            JsonError, JsonValue, json_array, json_array_bools, json_array_contains_prefix,
            json_array_contains_string, json_array_contains_substring, json_array_count_where,
            json_array_fold, json_array_get, json_array_ints, json_array_len, json_array_strings,
            json_as_bool, json_as_int, json_as_string, json_at, json_at_bool, json_at_bool_or,
            json_at_int, json_at_int_or, json_at_optional, json_at_optional_bool,
            json_at_optional_int, json_at_optional_string, json_at_or, json_at_string,
            json_at_string_or, json_at_to_string, json_at_to_string_or, json_bool_at,
            json_bool_at_or, json_bool_field, json_clone, json_decode_text, json_decode_value,
            json_error_message, json_field, json_field_bool, json_field_int, json_field_optional,
            json_field_optional_bool, json_field_optional_int, json_field_optional_string,
            json_field_string, json_int_at, json_int_at_or, json_int_field, json_is_array,
            json_is_null, json_is_object, json_kind, json_object, json_object_keys,
            json_object_len, json_parse, json_parse_file, json_quote_string, json_raw_field,
            json_string_array, json_string_at, json_string_at_or, json_string_field, json_strings,
            json_to_string, json_to_string_at, json_to_string_at_or, json_value, json_value_at,
            json_values, toml_parse_file, yaml_parse, yaml_parse_file,
        };
        #[allow(deprecated)]
        pub use crate::managed::{
            Managed, ManagedRead, ManagedWrite, WeakManaged, manage, manage_at, unwrap_runtime,
            unwrap_runtime_or_panic, weak,
        };
        pub use crate::math::{
            math_abs, math_abs_float, math_ceil, math_clamp, math_clamp_float, math_cos, math_exp,
            math_exp2, math_floor, math_log, math_log2, math_max, math_max_float, math_min,
            math_min_float, math_pow, math_pow_float, math_round, math_saturating_add,
            math_saturating_mul, math_saturating_sub, math_sin, math_sqrt, math_tanh,
            math_trunc_float, math_wrapping_add, math_wrapping_mul, math_wrapping_sub,
        };
        pub use crate::operation_context::OperationContext;
        pub use crate::process::{
            DEFAULT_RUNTIME_PROCESS_TIMEOUT_MS, ProcessEnv, ProcessEvent, ProcessOutput,
            ProcessRequest, RUNTIME_PROCESS_CONCURRENCY_CEILING,
            RUNTIME_PROCESS_OUTPUT_CEILING_BYTES, RUNTIME_PROCESS_TIMEOUT_CEILING_MS, args_all,
            args_count, args_get, args_get_or_default, log_error, log_error_json, log_trace,
            log_write, log_write_json, os_close, process_run, process_run_async,
            process_run_many_stdout, process_run_many_stdout_async,
            process_run_many_stdout_timeout, process_run_many_stdout_timeout_async,
            process_run_request, process_run_request_async, process_run_request_cancellable_async,
            process_run_stdout, process_run_stdout_async, process_run_stdout_timeout,
            process_run_stdout_timeout_async, process_run_timeout, process_run_timeout_async,
            process_stream,
        };
        pub use crate::random::{
            random_bool, random_bytes, random_float, random_int, random_string, uuid_new_v4,
        };
        pub use crate::regex::{
            RegexError, RssRegex, regex_captures, regex_compile, regex_error_message, regex_find,
            regex_is_match, regex_replace_all, regex_split,
        };
        pub use crate::resource_budget::{
            RUNTIME_ALLOCATION_CEILING_BYTES, ResourceBudget, ResourceBudgetError,
        };
        pub use crate::resource_pool::{
            PoolError, PoolStats, ResourceLease, ResourcePool, pool_error_message, pool_stats,
            pool_stats_available, pool_stats_capacity, pool_stats_created, pool_stats_in_use,
            resource_lease_discard,
        };
        #[cfg(feature = "net")]
        pub use crate::socket::{
            AllowAllNetworkTargetPolicy, DenyPrivateNetworkTargetPolicy, NetworkTargetPolicy,
            RssTcpStream, TcpError, tcp_connect, tcp_connect_with_context,
            tcp_connect_with_policy_and_context, tcp_error_message, tcp_stream_read,
            tcp_stream_read_with_context, tcp_stream_shutdown, tcp_stream_shutdown_with_context,
            tcp_stream_write, tcp_stream_write_all, tcp_stream_write_all_with_context,
            tcp_stream_write_with_context,
        };
        pub use crate::string_helpers::{
            char_compare, char_from_code, char_is_alpha, char_is_alphanumeric, char_is_digit,
            char_is_lower, char_is_upper, char_is_whitespace, char_to_code, char_to_lower,
            char_to_string, char_to_upper, float_is_finite, float_is_infinite, float_is_nan,
            float_to_string, int_bit_and, int_bit_not, int_bit_or, int_bit_xor, int_shift_left,
            int_shift_right, int_to_float, string_after, string_before, string_builder_finish,
            string_builder_new, string_builder_push, string_char_at, string_chars, string_concat,
            string_contains, string_copy, string_count, string_ends_with, string_format,
            string_from_bool, string_from_float, string_from_int, string_index_of, string_is_empty,
            string_join, string_len, string_lines, string_pad_left, string_pad_right,
            string_parse_float, string_parse_int, string_repeat, string_replace,
            string_replace_first, string_reverse, string_slice, string_split, string_starts_with,
            string_strip_prefix, string_to_lowercase, string_to_uppercase, string_trim,
            string_trim_end, string_trim_start, string_view, string_view_after, string_view_before,
            string_view_contains, string_view_is_empty, string_view_len, string_view_slice,
            string_view_starts_with, string_view_to_string,
        };
        pub use crate::tempdir::{
            TempDir, tempdir_keep, tempdir_new, tempdir_new_in, tempdir_path,
        };
        pub use crate::text_edit::{diff_unified, patch_apply_text};
        #[cfg(feature = "net")]
        pub use crate::websocket::{
            RssWebSocket, WebSocketError, websocket_close, websocket_close_with_context,
            websocket_connect, websocket_connect_with_context,
            websocket_connect_with_policy_and_context, websocket_error_message,
            websocket_recv_bytes, websocket_recv_bytes_with_context, websocket_recv_text,
            websocket_recv_text_with_context, websocket_send_bytes,
            websocket_send_bytes_with_context, websocket_send_text,
            websocket_send_text_with_context,
        };
    };
}

/// Exact compatibility surface used by generated RSScript Rust.
pub mod abi {
    runtime_abi_exports!();
}

runtime_abi_exports!();

/// Host integration APIs with explicit operation controls.
pub mod host {
    pub use crate::{
        NativeAsyncPending, OperationContext, ResourceBudget, ResourceBudgetError, RuntimeError,
        RuntimeErrorKind, SourceSpan, cancellation_never, cancellation_source_cancel,
        cancellation_source_new, cancellation_source_token, install_runtime_diagnostic_panic_hook,
        spawn_tokio_native, spawn_tokio_native_with_cancellation,
    };

    pub mod filesystem {
        pub use crate::{
            File, FileError, RuntimeBytes, RuntimePath, directory_create_all, directory_exists,
            directory_list_paths, file_open_read, file_open_write, file_read_all_with_budget,
            file_read_bytes_with_budget, file_read_string_with_budget, file_write_atomic,
            path_lexically_resolve_relative, path_lexically_safe_relative,
        };
    }

    pub mod process {
        pub use crate::{
            ProcessEnv, ProcessEvent, ProcessOutput, ProcessRequest, process_run_request_async,
            process_run_request_cancellable_async, process_stream,
        };
    }
}

/// Network APIs. Resource limits, cancellation, and deadlines use `OperationContext`.
#[cfg(feature = "net")]
pub mod net {
    pub mod policy {
        pub use crate::{
            AllowAllNetworkTargetPolicy, DenyPrivateNetworkTargetPolicy, NetworkTargetPolicy,
        };
    }

    pub mod http {
        pub use crate::{
            HttpError, HttpRequest, Response, http_error_message, http_get_async_with_context,
            http_request_json, http_request_with_header, http_request_with_retry,
            http_request_with_timeout, http_response_bytes, http_response_is_success,
            http_response_lines, http_response_status, http_response_text,
            http_send_async_with_context,
        };
    }

    pub mod tcp {
        pub use crate::{
            RssTcpStream, TcpError, tcp_connect_with_context, tcp_connect_with_policy_and_context,
            tcp_error_message, tcp_stream_read_with_context, tcp_stream_shutdown_with_context,
            tcp_stream_write_all_with_context, tcp_stream_write_with_context,
        };
    }

    pub mod websocket {
        pub use crate::{
            RssWebSocket, WebSocketError, websocket_close_with_context,
            websocket_connect_with_context, websocket_connect_with_policy_and_context,
            websocket_error_message, websocket_recv_bytes_with_context,
            websocket_recv_text_with_context, websocket_send_bytes_with_context,
            websocket_send_text_with_context,
        };
    }
}

/// Versioned canonical API for handwritten runtime consumers.
pub mod api {
    pub mod v1 {
        pub use crate::host;
        #[cfg(feature = "net")]
        pub use crate::net;

        pub mod data {
            pub use crate::{
                DecodeError, JsonError, JsonValue, base64_decode, base64_encode_bytes,
                gzip_decompress_bytes_with_budget, hash_sha256_bytes, hex_decode, hex_encode,
                json_parse, json_to_string, toml_parse_file, yaml_parse,
            };
        }

        pub mod time {
            pub use crate::{
                RssDeadline, RssDuration, RssInstant, clock_now, deadline_after, deadline_after_ms,
                deadline_is_expired, deadline_remaining_ms, duration_add, duration_as_ms,
                duration_ms, instant_elapsed,
            };
        }

        pub mod values {
            pub use crate::{
                Managed, RssPersistentMap, manage, map_get, map_insert, map_new, option_map,
                persistent_map_get, persistent_map_insert, persistent_map_new, result_map,
                set_insert, set_new,
            };
        }
    }
}
