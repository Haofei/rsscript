//! Internal package graph and health-check invariants.
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn package_graph_deduplicates_direct_and_transitive_path_dependencies() {
    let root_dir = common::unique_temp_dir("rsscript-package-dedup-root");
    let shared_dir = common::unique_temp_dir("rsscript-package-dedup-shared");
    let wrapper_dir = common::unique_temp_dir("rsscript-package-dedup-wrapper");
    common::write_named_package_fixture(
        &shared_dir,
        "rss-shared",
        "0.1.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    fs::create_dir_all(shared_dir.join("src")).expect("shared source dir should be created");
    fs::write(
        shared_dir.join("src/lib.rss"),
        r#"pub fn Shared.value() -> Int {
    return 1
}
"#,
    )
    .expect("shared source should be written");
    common::write_named_package_fixture(
        &wrapper_dir,
        "rss-wrapper",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            common::toml_path(&shared_dir)
        ),
        r#"pub fn Wrapper.value() -> Int
"#,
    );
    fs::create_dir_all(wrapper_dir.join("src")).expect("wrapper source dir should be created");
    fs::write(
        wrapper_dir.join("src/lib.rss"),
        r#"pub fn Wrapper.value() -> Int {
    return Shared.value()
}
"#,
    )
    .expect("wrapper source should be written");
    common::write_named_package_fixture(
        &root_dir,
        "rss-root",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
rss-wrapper = {{ path = "{}" }}
"#,
            common::toml_path(&shared_dir),
            common::toml_path(&wrapper_dir)
        ),
        r#"pub fn Root.value() -> Int
"#,
    );
    fs::create_dir_all(root_dir.join("src")).expect("root source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"pub fn Root.value() -> Int {
    return Shared.value() + Wrapper.value()
}
"#,
    )
    .expect("root source should be written");

    let check = check_package_dir(&root_dir).expect("package check should deduplicate path deps");
    let lock = lock_package_dir(&root_dir).expect("package lock should deduplicate path deps");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&shared_dir);
    let _ = fs::remove_dir_all(&wrapper_dir);

    assert!(check.graph.ok, "graph reasons: {:?}", check.graph.reasons);
    assert_eq!(
        lock.packages
            .iter()
            .filter(|package| package.name == "rss-shared")
            .count(),
        1
    );
}

#[test]
fn package_lowering_input_records_checked_in_native_abi_fixture() {
    let fixture_dir = common::workspace_root().join("packages/native-abi-fixture");

    let input = package_lowering_input(&fixture_dir).expect("fixture package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with native ABI fixture binding");

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(
        input.native_dependencies[0].crate_name,
        "rss_native_abi_fixture_bridge"
    );
    assert!(
        input.native_dependencies[0]
            .bindings
            .get("NativeAbiFixture.sort_int")
            .is_some_and(|target| target == "rss_native_abi_fixture_bridge::sort_int")
    );
    assert!(
        package
            .lib_rs
            .contains("rss_native_abi_fixture_bridge::sort_int")
    );
}

#[cfg(unix)]
#[test]
fn package_source_collection_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let dir = common::unique_temp_dir("rsscript-package-symlink");
    fs::create_dir_all(dir.join("interface")).expect("interface dir");
    fs::write(
        dir.join("rsspkg.toml"),
        "[package]\nname = \"sym\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interface\"]\n",
    )
    .expect("manifest");
    fs::write(dir.join("interface/lib.rssi"), "pub fn Sym.ok() -> Unit\n").expect("interface");

    // A secret outside the package the symlink will try to pull into review.
    let outside = common::unique_temp_dir("rsscript-package-symlink-outside");
    fs::create_dir_all(&outside).expect("outside dir");
    let secret = outside.join("secret.rssi");
    fs::write(&secret, "pub fn Secret.leak() -> Unit\n").expect("secret");
    symlink(&secret, dir.join("interface/evil.rssi")).expect("symlink");

    let result = review_package_dir(&dir);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&outside);

    let error = result.expect_err("a symlinked source file must be rejected, not followed");
    assert!(
        error.contains("symlink"),
        "expected a symlink rejection error, got: {error}"
    );
}
