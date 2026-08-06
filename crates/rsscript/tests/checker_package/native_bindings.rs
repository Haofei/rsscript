//! Spec §3/§9 — native wrappers and call bindings
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn native_rust_path_cannot_escape_the_package_root() {
    let temp_dir = common::unique_temp_dir("rsscript-package-native-path-escape");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "../outside"
crate = "outside"
"#,
        "",
    );

    let error = review_package_dir(&temp_dir)
        .expect_err("native paths outside the package must fail before review scanning");
    assert!(error.contains("escapes the package root"), "{error}");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[cfg(unix)]
#[test]
fn native_rust_symlink_cannot_escape_the_package_root() {
    use std::os::unix::fs::symlink;

    let temp_dir = common::unique_temp_dir("rsscript-package-native-symlink-escape");
    let outside = common::unique_temp_dir("rsscript-package-native-symlink-target");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "outside"
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("native")).expect("native parent should be created");
    fs::create_dir_all(&outside).expect("outside target should be created");
    symlink(&outside, temp_dir.join("native/rust")).expect("escape symlink should be created");

    let error = review_package_dir(&temp_dir)
        .expect_err("symlinked native paths outside the package must fail before scanning");
    assert!(
        error.contains("resolves outside the package root"),
        "{error}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn package_review_marks_async_native_await_boundary() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-async-native-boundary");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"
struct HostError

pub async fn Host.wait(ms: Int) -> Result<Unit, HostError>

pub async fn Api.run() -> Result<Unit, HostError>
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"
pub async fn Api.run() -> Result<Unit, HostError> {
    await Host.wait(ms: 1)?
    return Ok(Unit)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(json["await_sites"].as_array().is_some_and(|await_sites| {
        await_sites.iter().any(|site| {
            site["function"] == "Api.run"
                && site["callee"] == "Host.wait"
                && site["boundary"] == "native_pending"
        })
    }));
}

#[test]
fn package_review_json_counts_native_and_unsafe_apis_separately() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-api-effects");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
fn Native.danger(message: read String) -> String
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["native_apis"], 1);
    assert_eq!(json["summary"]["unsafe_apis"], 1);
}

#[test]
fn package_native_source_scan_ignores_comment_only_parallel_markers() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-native-comment-scan");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_comment_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.noop() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_comment_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "// use worker_pool::prelude::*;\n// std::thread::spawn(|| {});\npub fn noop() {}\n",
    )
    .expect("native source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(
        json["native_rust"]["semantic"]["source_scan_best_effort"]["worker_thread_parallelism_detected"],
        false
    );
}

#[test]
fn package_review_reir_marks_unbound_native_facade_external_binding_unknown() {
    let temp_dir =
        common::unique_temp_dir("rsscript-package-review-unbound-native-external_binding");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-report-upload"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[sources]
paths = ["src"]
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/s3.rssi"),
        r#"
fn S3.put_object(body: read String) -> Result<Unit, String>
"#,
    )
    .expect("interface should be written");
    fs::write(
        temp_dir.join("src/upload.rss"),
        r#"fn upload_report(report: read String) -> Result<Unit, String> {
    return S3.put_object(body: read report)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let review_json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let bundle: reir::Bundle =
        serde_json::from_str(&rsscript_review_reir::review_bundle_json(&review))
            .expect("package review REIR bundle should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(review.risk, rsscript::PackageRisk::Unknown);
    assert_eq!(review_json["summary"]["unknown_apis"], 1);
    assert!(review_json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native/external external_binding binding unknown")
    }));
    assert!(
        review_json["external_bindings"]
            .as_array()
            .is_some_and(|external_bindings| {
                external_bindings.iter().any(|external_binding| {
                    external_binding["function"] == "upload_report"
                        && external_binding["binding_symbol"] == "S3.put_object"
                        && external_binding["category"] == "unknown"
                        && external_binding["unknown_reason"]
                            == "native/external facade has no review.external_binding_bindings entry"
                        && external_binding["call_chain"].as_array().is_some_and(|chain| {
                            chain
                                == &vec![Value::from("upload_report"), Value::from("S3.put_object")]
                        })
                })
            })
    );

    assert!(bundle.facts.iter().any(|fact| {
        fact.kind == reir::FactKind::Capability
            && fact.role == Some(reir::FactRole::Required)
            && fact.value == reir::FactValue::Unknown
            && fact.subject.id == "rss-report-upload::function::upload_report"
            && fact.unknown_reason.as_deref()
                == Some("native/external facade has no review.external_binding_bindings entry")
            && fact.capability.as_ref().is_some_and(|external_binding| {
                external_binding.category == reir::CapabilityCategory::Unknown
                    && external_binding.action.as_deref() == Some("S3.put_object")
            })
    }));
}

#[test]
fn package_review_reir_records_native_build_time_execution() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-native-build-time-reir");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_build_native"
build_scripts = "review"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Build.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_build_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        "fn main() { println!(\"cargo:rerun-if-changed=build.rs\"); }\n",
    )
    .expect("native build script should be written");
    fs::write(temp_dir.join("native/rust/src/lib.rs"), "pub fn run() {}\n")
        .expect("native source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript_review_reir::review_bundle_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "build_time_execution"
                && fact["subject"]["kind"] == "build.step"
                && fact["subject"]["id"] == "rss-json@0.1.0::build::native_rust_build_script"
                && fact["confidence"]["level"] == "scanned"
                && fact["acquisition_mode"] == "source_scan"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "external_binding"
                && fact["external_binding"]["category"] == "build.execute"
                && fact["external_binding"]["service"] == "native_rust_source_scan"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "build_time_slice")
    }));
}

#[test]
fn package_review_json_records_selected_native_cargo_features() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-native-cargo-features");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[features]
wasm-browser = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_feature_native"
cargo_features = ["base-native"]
default-features = false
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"

[native.rust.feature_map]
wasm-browser = { cargo_features = ["worker-pool/web-spin-lock"] }
"#,
        r#"
fn Feature.value() -> Int
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_feature_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn value() -> i64 { 1 }\n",
    )
    .expect("native source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript_review_reir::review_bundle_json(&review))
            .expect("package review REIR JSON should parse");
    let lock = lock_package_dir(&temp_dir).expect("package lock should include native metadata");
    let _ = fs::remove_dir_all(&temp_dir);

    let cargo_features = json["native_rust"]["cargo_features"]
        .as_array()
        .expect("native cargo features should be an array");
    assert!(
        cargo_features
            .iter()
            .any(|feature| feature == "base-native")
    );
    assert!(
        cargo_features
            .iter()
            .any(|feature| feature == "worker-pool/web-spin-lock")
    );
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "native_cargo_feature"
                && fact["subject"]["id"] == "rss-json@0.1.0#native-cargo-feature:base-native"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "native_cargo_feature"
                && fact["subject"]["id"]
                    == "rss-json@0.1.0#native-cargo-feature:worker-pool/web-spin-lock"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "native_unsafe_slice")
    }));
    assert!(lock.packages[0].native_hash.is_some());
}

#[test]
fn package_check_fails_when_policy_denies_native_api() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-deny-native");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
deny_native = true
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "unknown");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package policy denies native public APIs")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "deny_native"
        })
    }));
}

#[test]
fn package_review_policy_maps_native_api_risk_to_elevated() {
    let temp_dir = common::unique_temp_dir("rsscript-package-native-api-risk");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
native_api_risk = "elevated"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["summary"]["native_apis"], 1);
}

#[test]
fn package_check_reports_invalid_native_rust_policy_values() {
    let temp_dir = common::unique_temp_dir("rsscript-package-invalid-native-rust-policy");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
build_scripts = "sometimes"
proc_macros = "never"
unsafe = "maybe"

[native.rust.policy]
ffi = "trusted"
wrapper_unsafe_blocks = "audited"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["summary"]["errors"], 6);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "build_scripts"
        }) && diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "proc_macros"
        }) && diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "PKG0501" && diagnostic["label"] == "unsafe")
            && diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == "PKG0501" && diagnostic["label"] == "ffi")
            && diagnostics.iter().any(|diagnostic| {
                diagnostic["code"] == "PKG0501" && diagnostic["label"] == "wrapper_unsafe_blocks"
            })
    }));
}

#[test]
fn package_check_applies_build_execution_default_to_native_wrapper() {
    let temp_dir = common::unique_temp_dir("rsscript-package-build-default-forbid");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
build_execution_default = "forbid"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
unsafe = "forbid"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    let _ = std::env::var("OUT_DIR");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_env_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script reads environment"))
    );
}

#[test]
fn package_review_uses_build_execution_default_as_native_review_policy() {
    let temp_dir = common::unique_temp_dir("rsscript-package-build-default-review");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
build_execution_default = "review"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
unsafe = "forbid"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["native_rust"]["build_scripts"], "review");
    assert_eq!(json["native_rust"]["proc_macros"], "review");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
            && reasons
                .iter()
                .any(|reason| reason == "native Rust proc macros require review")
    }));
}

#[test]
fn package_review_uses_nested_native_rust_policy() {
    let temp_dir = common::unique_temp_dir("rsscript-package-nested-native-policy");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"

[native.rust.policy]
build_scripts = "review"
proc_macros = "forbid"
wrapper_unsafe_blocks = "review"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["native_rust"]["build_scripts"], "review");
    assert_eq!(json["native_rust"]["proc_macros"], "forbid");
    assert_eq!(json["native_rust"]["unsafe_policy"], "review");
    assert_eq!(
        json["native_rust"]["unsafe_policies"]["wrapper_unsafe_blocks"],
        "review"
    );
    assert!(json["native_rust"]["unsafe_policies"]["rss_unsafe_apis"].is_null());
    assert!(json["native_rust"]["unsafe_policies"]["transitive_unsafe_blocks"].is_null());
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
            && reasons
                .iter()
                .any(|reason| reason == "native Rust unsafe policy requires review")
    }));
}

#[test]
fn package_review_preserves_all_native_unsafe_policy_dimensions() {
    let temp_dir = common::unique_temp_dir("rsscript-package-granular-native-policy");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"

[native.rust.policy]
rss_unsafe_apis = "allow"
wrapper_unsafe_blocks = "forbid"
transitive_unsafe_blocks = "review"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(
        json["native_rust"]["unsafe_policies"]["rss_unsafe_apis"],
        "allow"
    );
    assert_eq!(
        json["native_rust"]["unsafe_policies"]["wrapper_unsafe_blocks"],
        "forbid"
    );
    assert_eq!(
        json["native_rust"]["unsafe_policies"]["transitive_unsafe_blocks"],
        "review"
    );
}

#[test]
fn package_review_marks_unreadable_native_source_scan_incomplete() {
    let temp_dir = common::unique_temp_dir("rsscript-package-incomplete-native-scan");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
unsafe = "forbid"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native source directory");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml");
    fs::write(temp_dir.join("native/rust/src/lib.rs"), [0xff, 0xfe])
        .expect("invalid UTF-8 native source");

    let review = review_package_dir(&temp_dir).expect("incomplete scan should remain reportable");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert_eq!(
        json["native_rust"]["semantic"]["source_scan_best_effort"]["complete"],
        false
    );
    assert!(
        json["native_rust"]["semantic"]["source_scan_best_effort"]["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
    );
}

#[test]
fn package_review_applies_native_links_and_ffi_policy() {
    let temp_dir = common::unique_temp_dir("rsscript-package-native-links-ffi-policy");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_ffi_native"
links = ["z"]

[native.rust.policy]
native_links = "allow"
ffi = "forbid"
"#,
        r#"
fn Native.value() -> Int
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_ffi_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "extern \"C\" { fn abs(input: i32) -> i32; }\npub fn value() -> i64 { 1 }\n",
    )
    .expect("native source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["native_rust"]["native_links_policy"], "allow");
    assert_eq!(json["native_rust"]["ffi_policy"], "forbid");
    assert_eq!(
        json["native_rust"]["semantic"]["source_scan_best_effort"]["ffi_detected"],
        true
    );
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust FFI usage forbidden")
            && !reasons
                .iter()
                .any(|reason| reason == "native Rust links external libraries")
    }));
}

#[test]
fn package_check_reports_native_rust_consistency_issues() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["cargo_toml_present"], false);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], false);
    assert_eq!(json["native_rust"]["risk"], "high");
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust Cargo.toml missing"))
    );
}

#[test]
fn package_check_accepts_bound_native_interface_functions() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-binding");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"
fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.echo"
provider = "rss_json_native"
entry = "echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok, "{:?}", check.diagnostics);
    assert!(
        check
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "RS1301")
    );
}

#[test]
fn package_check_reports_unknown_native_binding_symbols() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-binding-unknown");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.ehco"
provider = "rss_json_native"
entry = "echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601" && diagnostic.label == "unknown native binding symbol"
    }));
}

#[test]
fn package_check_rejects_unsupported_native_binding_types() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-binding-type");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_config_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
struct Config

fn Native.load(config: read Config) -> Result<Int, Config>
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.load"
provider = "rss_config_native"
entry = "load"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_config_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn load() {}\n",
    )
    .expect("native source should be written");

    let check = check_package_dir(&temp_dir).expect("package check should complete");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601"
            && diagnostic.label == "unsupported native binding type"
            && diagnostic.summary.contains("parameter `config`")
    }));
}

#[test]
fn package_check_reports_native_binding_crate_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-binding-crate");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"
fn main() -> Unit {
    let message = Native.echo(message: read "hello native")
    Log.write(message: read message)
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.echo"
provider = "other_native"
entry = "echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601" && diagnostic.label == "native binding crate mismatch"
    }));
}

#[test]
fn package_check_reports_native_binding_without_native_rust() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-binding-no-native");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native")).expect("native dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.echo"
provider = "rss_json_native"
entry = "echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601"
            && diagnostic.label == "native binding without native Rust wrapper"
    }));
}

#[test]
fn package_check_reports_native_binding_missing_crate() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-binding-no-crate");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        "fn main() -> Unit { return Unit }\n",
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.echo"
provider = "rss_json_native"
entry = "echo"
"#,
    )
    .expect("native binding manifest should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert!(check.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PKG0601" && diagnostic.label == "native binding crate missing"
    }));
}

#[test]
fn package_check_reports_native_unsafe_usage() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-unsafe");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"

[native.rust.policy]
rss_unsafe_apis = "allow"
wrapper_unsafe_blocks = "forbid"
transitive_unsafe_blocks = "review"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        r#"pub fn parse() {
    let _ = "unsafe in a string";
    // unsafe in a comment should not count
    unsafe {}
}
"#,
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let native_path = temp_dir.join("native/rust").display().to_string();
    let reir_json: Value = serde_json::from_str(&format_package_check_reir_json(&check))
        .expect("package check REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["unsafe_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust unsafe usage detected"))
    );
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"] == "fact.package_check.rss_json_0_1_0.native"
                && fact["evidence"][0]["kind"] == "package_metadata"
                && fact["evidence"][0]["file"] == native_path
                && fact["evidence"][0]["json_pointer"] == "/native_rust"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "unsafe_boundary"
                && fact["id"] == "fact.package_check.rss_json_0_1_0.unsafe"
                && fact["evidence"][0]["file"] == native_path
                && fact["evidence"][0]["json_pointer"] == "/native_rust/unsafe_detected"
        })
    }));
}

#[test]
fn package_check_reports_native_linked_libraries() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-links");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
links = ["ssl"]
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok);
    assert_eq!(check.risk, rsscript::PackageRisk::Unknown);
    assert_eq!(
        json["native_rust"]["linked_libraries"],
        serde_json::json!(["ssl"])
    );
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust links external libraries")
    }));
}

#[test]
fn package_check_reports_native_build_script_environment_usage() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-build-env");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    let _ = std::env::var("OUT_DIR");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_env_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script reads environment"))
    );
}

#[test]
fn package_check_reports_native_build_script_download_risk() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-build-download");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/build.rs"),
        r#"fn main() {
    // https://example.invalid/commented-out should not be the only signal
    let _ = std::process::Command::new("curl").arg("https://example.invalid/archive.tar.gz");
}
"#,
    )
    .expect("native build script should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["native_rust"]["build_download_detected"], true);
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script may download code"))
    );
}

#[test]
fn package_lowering_input_records_native_wrapper_dependency() {
    let temp_dir = common::unique_temp_dir("rsscript-package-native-lowering-input");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");

    let input = package_lowering_input(&temp_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with native dependency");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(input.native_dependencies[0].crate_name, "rss_json_native");
    assert!(std::path::Path::new(&input.native_dependencies[0].path).ends_with("native/rust"));
    assert!(
        package
            .cargo_toml
            .contains("\"rss_json_native\" = { path = ")
    );
}

#[test]
fn package_lowering_input_passes_native_cargo_features_to_generated_cargo() {
    let temp_dir = common::unique_temp_dir("rsscript-package-native-lowering-features");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[features]
simd = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_feature_native"
cargo_features = ["base-native"]
default-features = false
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"

[native.rust.feature_map]
simd = { cargo_features = ["dep/simd"] }
"#,
        "",
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");

    let input = package_lowering_input(&temp_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with native dependency features");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(
        input.native_dependencies[0].cargo_features,
        vec!["base-native".to_string(), "dep/simd".to_string()]
    );
    assert!(!input.native_dependencies[0].default_features);
    assert!(
        package
            .cargo_toml
            .contains("\"rss_feature_native\" = { path = ")
    );
    assert!(
        package
            .cargo_toml
            .contains("features = [\"base-native\", \"dep/simd\"]")
    );
    assert!(package.cargo_toml.contains("default-features = false"));
}

#[test]
fn package_lowering_input_records_path_dependency_native_wrapper_dependency() {
    let root_dir = common::unique_temp_dir("rsscript-package-native-dep-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-native-dep-wrapper");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-native-dep",
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_dep_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"
fn Native.echo(message: read String) -> String
"#,
    );
    fs::create_dir_all(dep_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::create_dir_all(dep_dir.join("native")).expect("native dir should be created");
    fs::write(
        dep_dir.join("native/bindings.rssbind.toml"),
        r#"schema = "rsscript.bindings.v1"

[[function]]
symbol = "Native.echo"
provider = "rss_dep_native"
entry = "echo"
"#,
    )
    .expect("native bindings should be written");
    fs::write(
        dep_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_dep_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        dep_dir.join("native/rust/src/lib.rs"),
        "pub fn echo(message: &String) -> String { message.clone() }\n",
    )
    .expect("native source should be written");

    common::write_named_package_fixture(
        &root_dir,
        "rss-native-root",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-native-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"
fn main() -> Unit {
    let message = Native.echo(message: read "hello dep native")
    Log.write(message: read message)
    return Unit
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
    .expect("package source should lower with dependency native binding");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(input.native_dependencies[0].crate_name, "rss_dep_native");
    assert!(
        input.native_dependencies[0]
            .bindings
            .get("Native.echo")
            .is_some_and(|target| target == "rss_dep_native::echo")
    );
    assert!(
        input
            .interfaces
            .iter()
            .any(|(_, source)| source.contains("Native.echo"))
    );
    assert!(
        package
            .cargo_toml
            .contains("\"rss_dep_native\" = { path = ")
    );
    assert!(package.lib_rs.contains("rss_dep_native::echo"));
}

#[test]
fn package_lowering_input_records_checked_in_native_abi_fixture_binding() {
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
            .cargo_toml
            .contains("\"rss_native_abi_fixture_bridge\" = { path = ")
    );
    assert!(
        package
            .lib_rs
            .contains("rss_native_abi_fixture_bridge::sort_int")
    );
}
