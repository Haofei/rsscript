use std::{fs, path::Path};

use serde_json::Value;

#[test]
fn core_package_index_snapshot_is_current() {
    let snapshot_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("schemas/core-package-index.json");
    let snapshot =
        fs::read_to_string(snapshot_path).expect("core package index snapshot should exist");
    assert_eq!(snapshot, rsscript_sdk::core_package_index_json());
}

#[test]
fn async_surface_is_indexed_as_standard_package_not_default_core() {
    let index: Value = serde_json::from_str(rsscript_sdk::core_package_index_json())
        .expect("core package index should be valid json");
    let default_core = index["default_core"]
        .as_array()
        .expect("default_core should be an array");
    assert!(default_core.iter().all(|entry| {
        let path = entry["path"].as_str().unwrap_or_default();
        !path.starts_with("stdlib/async/") && !path.starts_with("stdlib/channel/")
    }));

    let packages = index["packages"]
        .as_array()
        .expect("packages should be an array");
    let async_package = packages
        .iter()
        .find(|entry| entry["name"] == "rss-async")
        .expect("rss-async package should be indexed");
    assert_eq!(async_package["path"], "packages/async");
    assert_eq!(
        async_package["interface_files"].as_array().map(Vec::len),
        Some(4)
    );
}
