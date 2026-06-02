use std::fs;

#[test]
fn core_package_index_snapshot_is_current() {
    let snapshot = fs::read_to_string("schemas/core-package-index.json")
        .expect("core package index snapshot should exist");
    assert_eq!(snapshot, rsscript::core_package_index_json());
}
