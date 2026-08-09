use rsscript_runtime::{abi, host};
use std::sync::Arc;

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
fn host_controls_and_root_abi_are_available_without_a_compatibility_facade() {
    let budget = host::ResourceBudget::new(16);
    let services = Arc::new(host::RuntimeServices::new().expect("runtime services"));
    let context = host::OperationContext::new(
        host::deadline_after_ms(1_000),
        host::cancellation_never(),
        budget,
        services,
    );
    assert_eq!(context.byte_budget().bytes_used(), 0);

    let encoded = rsscript_runtime::hex_encode(b"rss");
    assert_eq!(encoded, "727373");

    let mut values = rsscript_runtime::map_new();
    rsscript_runtime::map_insert(&mut values, &"key", &7);
    assert_eq!(rsscript_runtime::map_get(&values, &"key"), Some(7));
}
