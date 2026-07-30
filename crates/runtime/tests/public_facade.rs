use rsscript_runtime::{abi, api, host};

#[test]
fn generated_abi_root_and_module_remain_available() {
    assert_eq!(rsscript_runtime::string_len("abi"), 3);
    assert_eq!(abi::string_len("abi"), 3);

    let root: rsscript_runtime::JsonValue = rsscript_runtime::json_value("\"root\"");
    let namespaced: abi::JsonValue = abi::json_value("\"abi\"");
    assert_eq!(rsscript_runtime::json_to_string(&root), "\"root\"");
    assert_eq!(abi::json_to_string(&namespaced), "\"abi\"");
}

#[test]
fn canonical_api_is_domain_organized() {
    let budget = host::ResourceBudget::new(16);
    let context = host::OperationContext::new(
        api::v1::time::deadline_after_ms(1_000),
        host::cancellation_never(),
        budget,
    );
    assert_eq!(context.byte_budget().bytes_used(), 0);

    let encoded = api::v1::data::hex_encode(b"rss");
    assert_eq!(encoded, "727373");

    let mut values = api::v1::values::map_new();
    api::v1::values::map_insert(&mut values, &"key", &7);
    assert_eq!(api::v1::values::map_get(&values, &"key"), Some(7));
}

#[cfg(feature = "net")]
#[test]
fn network_facade_uses_operation_context_entrypoints() {
    use rsscript_runtime::net;

    let _http = net::http::http_send_async_with_context;
    let _tcp = net::tcp::tcp_connect_with_context;
    let _websocket = net::websocket::websocket_connect_with_context;
}
