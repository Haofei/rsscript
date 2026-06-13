//! Spec §2.5 — package health check (rss pkg / pkg ci)
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
fn package_lowering_input_records_checked_in_optional_core_packages() {
    let root_dir = common::unique_temp_dir("rsscript-package-optional-core-root");
    let repo = common::workspace_root();
    let cli_dir = repo.join("packages/core/cli");
    let crypto_dir = repo.join("packages/core/crypto");
    let sqlite_dir = repo.join("packages/adapters/sqlite");
    common::write_named_package_fixture(
        &root_dir,
        "rss-optional-core-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-core-cli = {{ path = "{}" }}
rss-core-crypto = {{ path = "{}" }}
rss-sqlite = {{ path = "{}" }}
"#,
            common::toml_path(&cli_dir),
            common::toml_path(&crypto_dir),
            common::toml_path(&sqlite_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Result<Unit, String> {
    let args = List<String>.new()
    List.push(list: mut args, value: read "--verbose")
    let verbose = Cli.flag(args: read args, name: read "verbose")
    Assert.equal_bool(left: verbose, right: true)

    let digest = Sha256.hex_string(value: read "abc")
    Assert.equal(left: read digest, right: read "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")

    let path = Path.from_string(value: read "target/rss-optional-core-app.db")
    Sqlite.execute(path: read path, sql: read "create table if not exists item(name text);")?
    let value = Sqlite.query_one_string(path: read path, sql: read "select name from item limit 1")?
    return Ok(Unit)
}
"#,
    )
    .expect("source should be written");

    let input = package_lowering_input(&root_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with optional core native bindings");
    let _ = fs::remove_dir_all(&root_dir);

    assert_eq!(input.native_dependencies.len(), 3);
    assert!(input.native_dependencies.iter().any(|dep| {
        dep.crate_name == "rss_cli_native"
            && dep
                .bindings
                .get("Cli.flag")
                .is_some_and(|target| target == "rss_cli_native::flag")
    }));
    assert!(input.native_dependencies.iter().any(|dep| {
        dep.crate_name == "rss_crypto_native"
            && dep
                .bindings
                .get("Sha256.hex_string")
                .is_some_and(|target| target == "rss_crypto_native::sha256_hex_string")
    }));
    assert!(input.native_dependencies.iter().any(|dep| {
        dep.crate_name == "rss_sqlite_native"
            && dep
                .bindings
                .get("Sqlite.execute")
                .is_some_and(|target| target == "rss_sqlite_native::execute")
    }));
    assert!(package.lib_rs.contains("rss_cli_native::flag"));
    assert!(
        package
            .lib_rs
            .contains("rss_crypto_native::sha256_hex_string")
    );
    assert!(package.lib_rs.contains("rss_sqlite_native::execute"));
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
