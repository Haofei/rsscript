//! Spec §2.5 — package semantic diff (rss pkg diff)
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn package_diff_reports_manifest_and_interface_contract_changes() {
    let old_dir = common::unique_temp_dir("rsscript-package-diff-old");
    let new_dir = common::unique_temp_dir("rsscript-package-diff-new");
    common::write_package_fixture(
        &old_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    common::write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[dependencies]
rss-core = "0.5"
rss-cache = "0.1"

[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
"#,
        r#"features: native

struct JsonValue
struct JsonError

pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["new_package"]["version"], "0.2.0");
    assert_eq!(json["risk"], "unknown");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "dependency" && change["name"] == "rss-cache")
    }));
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["kind"] == "native-rust" && change["name"] == "build_scripts")
    }));
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["file"] == "interface/lib.rssi" && change["risk"] == "high")
    }));
}

#[test]
fn package_diff_can_emit_reir_diff_json() {
    let old_dir = common::unique_temp_dir("rsscript-package-reir-diff-old");
    let new_dir = common::unique_temp_dir("rsscript-package-reir-diff-new");
    common::write_package_fixture(
        &old_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run(value: read Int) -> Int
"#,
    );
    common::write_package_fixture(
        &new_dir,
        "0.1.0",
        "",
        r#"features: native

pub fn NativeBridge.run(value: read Int) -> Int
    effects(native)
"#,
    );

    let old_review = review_package_dir(&old_dir).expect("old package review should succeed");
    let new_review = review_package_dir(&new_dir).expect("new package review should succeed");
    let json: Value = serde_json::from_str(&format_package_review_reir_diff_json(
        &old_review,
        &new_review,
    ))
    .expect("REIR diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["schema"], "reir.diff.v0.1");
    assert!(json["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "fact_added"
                && item["subject"]["id"] == "rss-json::native::NativeBridge"
        })
    }));
}

#[test]
fn package_diff_marks_added_boundary_interface_files_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-added-boundary-interface-old");
    let new_dir = common::unique_temp_dir("rsscript-package-added-boundary-interface-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-added-interface",
        "0.1.0",
        "",
        r#"struct Cache
struct Bytes

pub fn Cache.get(cache: read Cache) -> Bytes
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-added-interface",
        "0.1.0",
        "",
        r#"struct Cache
struct Bytes

pub fn Cache.get(cache: read Cache) -> Bytes
"#,
    );
    fs::write(
        new_dir.join("interface/retention.rssi"),
        r#"pub fn Cache.put(cache: mut Cache, value: read Bytes) -> Unit
    effects(retains(value))
"#,
    )
    .expect("added boundary interface should be written");

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "high-risk interface change detected")
    }));
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/retention.rssi"
                && change["change"] == "added"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_added_boundary_contracts_in_modified_interface_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-modified-boundary-interface-old");
    let new_dir = common::unique_temp_dir("rsscript-package-modified-boundary-interface-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-modified-interface",
        "0.1.0",
        "",
        r#"features: native

struct Bytes

pub fn Bytes.len(value: read Bytes) -> Int
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-modified-interface",
        "0.1.0",
        "",
        r#"features: native

struct Bytes

pub fn Bytes.len(value: read Bytes) -> Int

native fn Bytes.decode(value: read Bytes) -> String
    effects(native)
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "unknown");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_handle_field_contract_changes_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-handle-field-interface-old");
    let new_dir = common::unique_temp_dir("rsscript-package-handle-field-interface-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-handle-interface",
        "0.1.0",
        "",
        r#"class Rules

struct Config {
    rules: Rules
}
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-handle-interface",
        "0.1.0",
        "",
        r#"class Rules

struct Config {
    rules: handle Rules
}
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_noescape_callback_contract_changes_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-noescape-interface-old");
    let new_dir = common::unique_temp_dir("rsscript-package-noescape-interface-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-noescape-interface",
        "0.1.0",
        "",
        r#"pub fn Scheduler.run(callback: Closure) -> Unit
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-noescape-interface",
        "0.1.0",
        "",
        r#"pub fn Scheduler.run(callback: noescape Fn()) -> Unit
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_marks_protocol_impl_mapping_changes_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-protocol-impl-old");
    let new_dir = common::unique_temp_dir("rsscript-package-protocol-impl-new");
    let old_interface = r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

pub fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))
pub fn BufferWriter.audit_write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

impl Writer for BufferWriter {
    write = BufferWriter.write
}
"#;
    let new_interface = r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

pub fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))
pub fn BufferWriter.audit_write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

impl Writer for BufferWriter {
    write = BufferWriter.audit_write
}
"#;
    common::write_named_package_fixture(&old_dir, "rss-protocol-diff", "0.1.0", "", old_interface);
    common::write_named_package_fixture(&new_dir, "rss-protocol-diff", "0.1.0", "", new_interface);

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["file"] == "interface/lib.rssi"
                && change["change"] == "modified"
                && change["risk"] == "high"
                && change["findings"].as_array().is_some_and(|findings| {
                    findings.iter().any(|finding| finding["code"] == "RSR016")
                })
        })
    }));
}

#[test]
fn package_diff_preserves_callback_result_contract_types() {
    let old_dir = common::unique_temp_dir("rsscript-package-fn-result-interface-old");
    let new_dir = common::unique_temp_dir("rsscript-package-fn-result-interface-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-fn-result-interface",
        "0.1.0",
        "",
        r#"struct Scheduler
struct BuildError

pub fn Scheduler.run(callback: read Fn(Int) -> Result<String, BuildError>) -> Unit
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-fn-result-interface",
        "0.1.0",
        "",
        r#"struct Scheduler
struct BuildError

pub fn Scheduler.run(callback: read Fn(Int) -> Result<Int, BuildError>) -> Unit
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json = rsscript::format_package_diff_json(&diff);
    let parsed: Value = serde_json::from_str(&json).expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(parsed["risk"], "elevated");
    assert!(
        json.contains("callback: read Fn(Int) -> Result<String, BuildError>"),
        "{json}"
    );
    assert!(
        json.contains("callback: read Fn(Int) -> Result<Int, BuildError>"),
        "{json}"
    );
}

#[test]
fn package_diff_marks_async_api_changes_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-async-interface-old");
    let new_dir = common::unique_temp_dir("rsscript-package-async-interface-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-async-interface",
        "0.1.0",
        "",
        r#"struct TimerError

pub fn Api.run() -> Result<Unit, TimerError>
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-async-interface",
        "0.1.0",
        "",
        r#"features: async

struct TimerError

pub async fn Api.run() -> Result<Unit, TimerError>
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["interface_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["risk"] == "high"
                && change["findings"].as_array().is_some_and(|findings| {
                    findings.iter().any(|finding| finding["code"] == "RSR001")
                })
        })
    }));
}

#[test]
fn package_diff_marks_boundary_package_feature_changes_high_risk() {
    let old_dir = common::unique_temp_dir("rsscript-package-feature-diff-old");
    let new_dir = common::unique_temp_dir("rsscript-package-feature-diff-new");
    common::write_named_package_fixture(
        &old_dir,
        "rss-feature-diff",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    common::write_named_package_fixture(
        &new_dir,
        "rss-feature-diff",
        "0.1.0",
        r#"[features]
native-tls = ["native"]
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["manifest_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["kind"] == "package-feature"
                && change["name"] == "native-tls"
                && change["risk"] == "high"
        })
    }));
}

#[test]
fn package_diff_reports_await_site_metadata_changes() {
    let old_dir = common::unique_temp_dir("rsscript-package-diff-await-old");
    let new_dir = common::unique_temp_dir("rsscript-package-diff-await-new");
    let interface = r#"features: async, native

struct TimerError

pub async native fn Timer.sleep(ms: Int) -> Result<Unit, TimerError>
    effects(native)

pub async fn Api.run() -> Result<Unit, TimerError>
"#;
    common::write_named_package_fixture(&old_dir, "rss-async-diff", "0.1.0", "", interface);
    common::write_named_package_fixture(&new_dir, "rss-async-diff", "0.1.0", "", interface);
    fs::create_dir_all(old_dir.join("src")).expect("old src dir should be created");
    fs::write(
        old_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run() -> Result<Unit, TimerError> {
    return Ok(Unit)
}
"#,
    )
    .expect("old source should be written");
    fs::create_dir_all(new_dir.join("src")).expect("new src dir should be created");
    fs::write(
        new_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}
"#,
    )
    .expect("new source should be written");

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let human = rsscript::format_package_diff_human(&diff);
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "await site review metadata changed")
    }));
    assert!(matches!(
        json["risk"].as_str(),
        Some("elevated" | "high" | "unknown")
    ));
    assert!(human.contains("await sites: 0 -> 1"), "{human}");
}

#[test]
fn package_diff_ignores_unchanged_await_site_directory_paths() {
    let old_dir = common::unique_temp_dir("rsscript-package-diff-await-same-old");
    let new_dir = common::unique_temp_dir("rsscript-package-diff-await-same-new");
    let interface = r#"features: async, native

struct TimerError

pub async native fn Timer.sleep(ms: Int) -> Result<Unit, TimerError>
    effects(native)

pub async fn Api.run() -> Result<Unit, TimerError>
"#;
    let source = r#"features: async

pub async fn Api.run() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}
"#;
    common::write_named_package_fixture(&old_dir, "rss-async-diff", "0.1.0", "", interface);
    common::write_named_package_fixture(&new_dir, "rss-async-diff", "0.1.0", "", interface);
    fs::create_dir_all(old_dir.join("src")).expect("old src dir should be created");
    fs::create_dir_all(new_dir.join("src")).expect("new src dir should be created");
    fs::write(old_dir.join("src/main.rss"), source).expect("old source should be written");
    fs::write(new_dir.join("src/main.rss"), source).expect("new source should be written");

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert!(
        !json["reasons"].as_array().is_some_and(|reasons| {
            reasons
                .iter()
                .any(|reason| reason == "await site review metadata changed")
        }),
        "{json}"
    );
}

#[test]
fn package_diff_preserves_duplicate_await_site_counts() {
    let old_dir = common::unique_temp_dir("rsscript-package-diff-await-count-old");
    let new_dir = common::unique_temp_dir("rsscript-package-diff-await-count-new");
    let interface = r#"features: async, native

struct TimerError

pub async native fn Timer.sleep(ms: Int) -> Result<Unit, TimerError>
    effects(native)

pub async fn Api.run() -> Result<Unit, TimerError>
"#;
    common::write_named_package_fixture(&old_dir, "rss-async-diff", "0.1.0", "", interface);
    common::write_named_package_fixture(&new_dir, "rss-async-diff", "0.1.0", "", interface);
    fs::create_dir_all(old_dir.join("src")).expect("old src dir should be created");
    fs::write(
        old_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}
"#,
    )
    .expect("old source should be written");
    fs::create_dir_all(new_dir.join("src")).expect("new src dir should be created");
    fs::write(
        new_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}
"#,
    )
    .expect("new source should be written");

    let diff = diff_package_dirs(&old_dir, &new_dir).expect("package diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_diff_json(&diff))
        .expect("package diff JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "await site review metadata changed")
    }));
}

#[test]
fn package_lock_diff_marks_boundary_feature_selection_high_risk() {
    let lock_dir = common::unique_temp_dir("rsscript-package-lock-boundary-feature");
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    let old_lock = rsscript::PackageLock {
        version: 1,
        packages: vec![rsscript::PackageLockPackage {
            name: "rss-net".to_string(),
            version: "0.1.0".to_string(),
            source: "path:.".to_string(),
            checksum: "sha256:old".to_string(),
            interface_hash: "sha256:interface".to_string(),
            review_hash: "sha256:review".to_string(),
            native_hash: None,
            features: Vec::new(),
        }],
        metadata: rsscript::PackageLockMetadata {
            rsscript_version: "0.5".to_string(),
            created_by: "test".to_string(),
        },
    };
    let mut new_lock = old_lock.clone();
    new_lock.packages[0].features = vec!["native-tls".to_string()];
    fs::write(&old_lock_path, format_package_lock_toml(&old_lock))
        .expect("old lock should be written");
    fs::write(&new_lock_path, format_package_lock_toml(&new_lock))
        .expect("new lock should be written");

    let diff =
        diff_package_locks(&old_lock_path, &new_lock_path).expect("lock diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_diff_json(&diff))
        .expect("lock diff JSON should parse");
    let _ = fs::remove_dir_all(&lock_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package feature selection changed")
    }));
    assert!(json["package_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["name"] == "rss-net"
                && change["risk"] == "high"
                && change["changes"].as_array().is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| field["field"] == "features" && field["risk"] == "high")
                })
        })
    }));
}
