mod common;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rsscript::{
    check_package_dir, diff_package_dirs, diff_package_locks, format_package_lock_toml,
    format_package_review_reir_diff_json, format_package_review_reir_json, lock_package_dir,
    lower_sources_to_rust_package_with_options, package_lowering_input, package_metadata,
    package_metadata_verify, package_tree, publish_package_dry_run, review_package_dir,
    vendor_package_dir,
};
use serde_json::Value;

fn mock_iam_grant(action: &str) -> reir::Fact {
    reir::Fact {
        schema: "reir.fact.v0.1".to_string(),
        id: format!("fact.mock_iam.grant.{}", action.replace(':', "_")),
        kind: reir::FactKind::Capability,
        role: Some(reir::FactRole::Granted),
        subject: reir::Subject {
            kind: reir::SubjectKind::CloudRole,
            id: "arn:aws:iam::123456789012:role/report-uploader".to_string(),
            name: Some("report-uploader".to_string()),
            package: None,
        },
        capability: Some(reir::Capability {
            category: reir::CapabilityCategory::ObjectStorageWrite,
            provider: Some("aws".to_string()),
            service: Some("s3".to_string()),
            action: Some(action.to_string()),
            resource: Some("arn:aws:s3:::reports-prod/*".to_string()),
            constraints: HashMap::new(),
        }),
        value: reir::FactValue::True,
        confidence: reir::Confidence {
            level: reir::ConfidenceLevel::Authoritative,
            source: Some("mock_iam".to_string()),
        },
        acquisition_mode: reir::AcquisitionMode::CloudPolicy,
        precision: reir::Precision::ResourceScoped,
        evidence: Vec::new(),
        unknown_reason: None,
    }
}

#[test]
fn package_review_reads_manifest_and_reports_semantic_risk() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-json"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[sources]
paths = ["src"]

[dependencies]
rss-core = "0.5"

[features]
streaming = []

[review.expect]
risk = "low"

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "review"
proc_macros = "forbid"
unsafe = "forbid"
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/json.rssi"),
        r#"features: native

struct JsonValue
struct JsonError

native fn Json.parse(text: read String) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    )
    .expect("interface should be written");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn helper(text: read String) -> String {
    return text
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["package"]["name"], "rss-json");
    assert_eq!(json["risk"], "high");
    assert_eq!(json["features"], serde_json::json!(["streaming"]));
    assert_eq!(
        json["dependencies"],
        serde_json::json!([
            {
                "name": "rss-core",
                "requirement": "0.5",
                "source": "registry",
                "features": [],
                "dependency_kind": "normal",
                "compile_only": false,
                "test_only": false,
                "platform_provided": false
            }
        ])
    );
    assert_eq!(json["summary"]["interface_files"], 1);
    assert_eq!(json["summary"]["source_files"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust build scripts require review")
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native Rust wrapper enabled")
    }));
    assert!(human.contains("package features: streaming"));
    assert!(human.contains("dependency rss-core registry requirement 0.5"));
}

#[test]
fn package_review_can_emit_reir_bundle_json() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-reir");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[dependencies]
rss-core = "0.5"
"#,
        r#"features: native

module rss.package.review

use rss.package.contract.PackageContract
use rss.review.ReviewMap

pub fn NativeBridge.run(value: read Int) -> Int
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let bundle: Value = serde_json::from_str(&format_package_review_reir_json(&review))
        .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(bundle["schema"], "reir.bundle.v0.1");
    assert_eq!(bundle["ontology"], "reir.capability_ontology.v0.1");
    assert!(bundle["facts"].as_array().is_some_and(|facts| {
        facts
            .iter()
            .any(|fact| fact["kind"] == "package_risk" && fact["subject"]["id"] == "rss-json@0.1.0")
            && facts.iter().any(|fact| {
                fact["kind"] == "dependency_risk"
                    && fact["subject"]["id"] == "rss-core@0.5"
                    && fact["value"] == "unknown"
            })
            && facts.iter().any(|fact| {
                fact["kind"] == "native_boundary"
                    && fact["subject"]["id"] == "rss-json::native::NativeBridge"
            })
            && facts.iter().any(|fact| {
                fact["kind"] == "native_module_declaration"
                    && fact["subject"]["id"] == "rss-json::native::NativeBridge"
            })
            && facts.iter().any(|fact| {
                fact["kind"] == "module_declaration"
                    && fact["subject"]["id"] == "rss-json::module::rss.package.review"
            })
            && facts.iter().any(|fact| {
                fact["kind"] == "use_declaration"
                    && fact["subject"]["id"] == "rss-json::module::rss.package.review"
                    && fact["evidence"].as_array().is_some_and(|evidence| {
                        evidence
                            .iter()
                            .any(|item| item["symbol"] == "rss.package.contract.PackageContract")
                    })
            })
            && facts.iter().any(|fact| {
                fact["kind"] == "use_declaration"
                    && fact["subject"]["id"] == "rss-json::module::rss.package.review"
                    && fact["evidence"].as_array().is_some_and(|evidence| {
                        evidence
                            .iter()
                            .any(|item| item["symbol"] == "rss.review.ReviewMap")
                    })
            })
    }));
    assert!(bundle["edges"].as_array().is_some_and(|edges| {
        edges.iter().any(|edge| edge["kind"] == "crosses_native")
            && edges
                .iter()
                .any(|edge| edge["kind"] == "depends_on" && edge["to"]["id"] == "rss-core@0.5")
            && edges.iter().any(|edge| {
                edge["kind"] == "normalizes_to_native_fn"
                    && edge["from"]["id"] == "rss-json::native::NativeBridge"
                    && edge["to"]["id"] == "rss-json::NativeBridge.run"
            })
    }));
    assert!(bundle["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "package_risk_slice")
            && slices
                .iter()
                .any(|slice| slice["kind"] == "native_unsafe_slice")
    }));
}

#[test]
fn package_review_reports_feature_boundary_risk() {
    let temp_dir = common::unique_temp_dir("rsscript-package-feature-risk");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-feature-risk",
        "0.1.0",
        r#"[features]
native-tls = ["native"]
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "high");
    assert_eq!(json["features"], serde_json::json!(["native-tls"]));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason == "package feature `native-tls` may change native/unsafe/build risk"
        })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "package_feature"
                && fact["subject"]["id"] == "rss-feature-risk@0.1.0#feature:native-tls"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "package_feature_slice")
    }));
}

#[test]
fn package_review_selects_feature_conditioned_interface_paths() {
    let temp_dir = common::unique_temp_dir("rsscript-package-feature-interface");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-feature-interface",
        "0.1.0",
        r#"[features]
streaming = []

[interfaces.features.streaming]
paths = ["interface/streaming"]
"#,
        r#"pub fn Json.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("interface/streaming"))
        .expect("feature interface dir should be created");
    fs::write(
        temp_dir.join("interface/streaming/lib.rssi"),
        r#"pub fn Json.stream(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["interface_files"], 2);
    assert_eq!(json["summary"]["public_functions"], 2);
    assert!(
        json["exports"].as_array().is_some_and(|exports| {
            exports.iter().any(|export| export["name"] == "Json.stream")
        })
    );
}

#[test]
fn package_review_loads_path_dependency_interfaces_for_source_checks() {
    let root_dir = common::unique_temp_dir("rsscript-package-dep-interface-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-dep-interface-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.parse(text: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(review.summary.source_files, 1);
    assert_eq!(review.diagnostics, Vec::new());
}

#[test]
fn package_review_uses_selected_dependency_feature_interfaces_for_source_checks() {
    let root_dir = common::unique_temp_dir("rsscript-package-dep-feature-interface-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-dep-feature-interface-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = ["simd"]
simd = []

[interfaces.features.fast]
paths = ["interface/fast"]

[interfaces.features.simd]
paths = ["interface/simd"]
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(dep_dir.join("interface/fast"))
        .expect("feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/fast/lib.rssi"),
        r#"pub fn Dep.fast(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");
    fs::create_dir_all(dep_dir.join("interface/simd"))
        .expect("transitive feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/simd/lib.rssi"),
        r#"pub fn Dep.simd(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            common::toml_path(&dep_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.simd(text: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(review.diagnostics, Vec::new());
}

#[test]
fn package_check_reports_unknown_selected_dependency_feature() {
    let root_dir = common::unique_temp_dir("rsscript-package-dep-unknown-feature-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-dep-unknown-feature-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["turbo"] }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0101" && diagnostic["label"] == "unknown package feature"
        })
    }));
}

#[test]
fn package_check_rejects_git_dependency_source() {
    let temp_dir = common::unique_temp_dir("rsscript-package-git-dependency-source");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-app",
        "0.1.0",
        r#"[dependencies]
rss-remote = { git = "https://example.invalid/rss-remote.git", rev = "abc123" }
"#,
        r#"pub fn App.run() -> Unit
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
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0102"
                && diagnostic["label"] == "unsupported dependency source"
        })
    }));
}

#[test]
fn package_check_rejects_denied_resolved_dependency_feature() {
    let root_dir = common::unique_temp_dir("rsscript-package-denied-feature-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-denied-feature-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = ["unsafe-backend"]
unsafe-backend = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}

[review.feature_policy]
deny = ["*/unsafe-backend"]
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "denied package feature"
        })
    }));
}

#[test]
fn package_check_reports_provider_implementation_declaration() {
    let temp_dir = common::unique_temp_dir("rsscript-package-provider-implementation");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-platform-provider",
        "0.1.0",
        r#"[implements."platform-env"]
interface_features = ["posix"]
"#,
        r#"pub fn Env.home() -> String
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
    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let check_reir_json: Value =
        serde_json::from_str(&rsscript::format_package_check_reir_json(&check))
            .expect("package check REIR JSON should parse");
    let manifest_path = temp_dir.join("rsspkg.toml").display().to_string();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["implements"][0]["interface_package"], "platform-env");
    assert_eq!(json["implements"][0]["interface_features"][0], "posix");
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "PKG0901" && diagnostic["label"] == "version")
            && diagnostics.iter().any(|diagnostic| {
                diagnostic["code"] == "PKG0901" && diagnostic["label"] == "interface_effective_hash"
            })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "provider_implementation"
                && fact["subject"]["id"] == "rss-platform-provider@0.1.0::implements::platform-env"
                && fact["value"] == "unknown"
        })
    }));
    assert!(check_reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "provider_implementation"
                && fact["subject"]["id"] == "rss-platform-provider@0.1.0::implements::platform-env"
                && fact["evidence"][0]["source"] == "rsscript_package_check"
                && fact["evidence"][0]["file"] == manifest_path
                && fact["evidence"][0]["json_pointer"] == "/implements/0"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "provider_implementation_slice")
    }));
}

#[test]
fn package_review_reports_missing_interface_implementation() {
    let temp_dir = common::unique_temp_dir("rsscript-package-missing-interface-impl");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-missing-impl",
        "0.1.0",
        "",
        r#"pub fn render(body: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn helper(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(!check.ok);
}

#[test]
fn package_review_reports_interface_implementation_signature_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-interface-mismatch");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-interface-mismatch",
        "0.1.0",
        "",
        r#"pub fn render(body: read String) -> fresh String
    effects(no_panic)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub fn render(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(
        causes.iter().any(|cause| cause.contains(
            "interface: pub fn render(body: read String) -> fresh String effects(no_panic)"
        )),
        "{causes:?}"
    );
    assert!(
        causes
            .iter()
            .any(|cause| cause.contains("source: pub fn render(body: read String) -> String")),
        "{causes:?}"
    );
}

#[test]
fn package_review_reports_missing_interface_type_declaration() {
    let temp_dir = common::unique_temp_dir("rsscript-package-missing-interface-type");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-missing-type",
        "0.1.0",
        "",
        r#"struct PublicConfig {
    name: String
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn main() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(!check.ok);
}

#[test]
fn package_review_reports_interface_type_contract_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-interface-type-mismatch");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-type-mismatch",
        "0.1.0",
        "",
        r#"class Session<T: Managed> {
    user: handle User
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub struct Session<T: Managed> {
    user: User
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(codes.contains(&"RS1301"), "{codes:?}");
    assert!(
        causes
            .iter()
            .any(|cause| cause
                .contains("interface: class Session<T: Managed> { user: handle User }")),
        "{causes:?}"
    );
    assert!(
        causes
            .iter()
            .any(|cause| cause.contains("source: struct Session<T: Managed> { user: User }")),
        "{causes:?}"
    );
}

#[test]
fn package_review_reports_interface_data_model_contract_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-interface-data-model-mismatch");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-data-model-mismatch",
        "0.1.0",
        "",
        r#"sum PackageError {
    Io(path: String),
    Invalid
}

type PackageName = String

const MAX_RETRIES: Int = 3
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub sum PackageError {
    Io(code: Int),
    Invalid
}

pub type PackageName = Bytes

pub const MAX_RETRIES: Int = 4
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let labels = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.label.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        labels.contains(&"interface/source sum type mismatch"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"interface/source type alias mismatch"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"interface/source const mismatch"),
        "{labels:?}"
    );
}

#[test]
fn package_review_rejects_namespace_interface_shorthand() {
    let temp_dir = common::unique_temp_dir("rsscript-package-namespace-opaque-interface");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-namespace-opaque",
        "0.1.0",
        r#"[sources]
paths = ["src"]
"#,
        r#"namespace Json

opaque struct JsonValue
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"struct Json.JsonValue {
    text: String
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should complete");
    let _ = fs::remove_dir_all(&temp_dir);

    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"RS0015"), "{:?}", review.diagnostics);
}

#[test]
fn package_review_reports_path_dependency_interface_call_violations() {
    let root_dir = common::unique_temp_dir("rsscript-package-dep-interface-violation-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-dep-interface-violation-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/lib.rss"),
        r#"fn render(body: read String) -> String {
    return Dep.parse(value: read body)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(codes.contains(&"RS0203"), "{codes:?}");
    assert!(codes.contains(&"RS0204"), "{codes:?}");
    assert!(!codes.contains(&"RS0206"), "{codes:?}");
}

#[test]
fn package_review_reports_dependency_interface_symbol_conflicts_without_sources() {
    let root_dir = common::unique_temp_dir("rsscript-package-interface-conflict-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-interface-conflict-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Shared.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn Shared.parse(text: read String) -> String
"#,
    );

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let codes = review
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(codes.contains(&"RS0005"), "{codes:?}");
}

#[test]
fn package_review_includes_lint_warnings_for_public_contracts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-lint");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: native

struct Error

pub fn Api.overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Image>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Image>>>, Error>
    effects(no_panic, noalloc, no_block, pure, native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["errors"], 0);
    assert_eq!(json["summary"]["guarantee_apis"], 1);
    assert_eq!(json["summary"]["native_guarantee_apis"], 1);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.overloaded"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "review-only guarantee `pure` on native boundary")
                })
        })
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package contains frontend warnings")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "RSL001"
                && diagnostic["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("7 parameters"))
        })
    }));
}

#[test]
fn package_review_summarizes_async_apis_and_await_sites() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-async");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: async, native

struct TimerError
struct Client

pub async native fn Timer.sleep(ms: Int) -> Result<Unit, TimerError>
    effects(native)

pub fn Log.done(client: read Client) -> Unit

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError>
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    Log.done(client: read client)
    return Ok(Unit)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["async_apis"], 2);
    assert_eq!(json["summary"]["await_sites"], 1);
    assert!(json["await_sites"].as_array().is_some_and(|await_sites| {
        await_sites.iter().any(|site| {
            site["function"] == "Api.run"
                && site["callee"] == "Timer.sleep"
                && site["boundary"] == "runtime_pending"
                && site["live_across_await"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value == "client"))
                && site["span"]["file"]
                    .as_str()
                    .is_some_and(|file| file.ends_with("src/main.rss"))
        })
    }));
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["function_kind"] == "async"
                && export["normalized_effects"]
                    .as_array()
                    .is_some_and(|effects| effects.iter().any(|effect| effect == "suspends"))
                && export["reasons"]
                    .as_array()
                    .is_some_and(|reasons| reasons.iter().any(|reason| reason == "async boundary"))
        })
    }));
    assert!(
        human.contains("await sites:") && human.contains("Api.run awaits Timer.sleep"),
        "{human}"
    );
    assert!(human.contains("live_across [client]"), "{human}");
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "async_boundary"
                && fact["subject"]["id"] == "rss-json::Api.run"
                && fact["evidence"].as_array().is_some_and(|evidence| {
                    evidence.iter().any(|item| {
                        item["reason"].as_str().is_some_and(|reason| {
                            reason.contains("boundary=runtime_pending")
                                && reason.contains("callee=Timer.sleep")
                        })
                    })
                })
        })
    }));
    assert!(
        reir_json["slices"]
            .as_array()
            .is_some_and(|slices| { slices.iter().any(|slice| slice["kind"] == "async_slice") })
    );
}

#[test]
fn package_review_resolves_task_group_async_let_await_callees() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-task-group-await");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: async, native

struct TimerError
struct Client

pub async native fn Timer.sleep(ms: Int) -> Result<Unit, TimerError>
    effects(native)

pub fn Log.done(client: read Client) -> Unit

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError>
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError> {
    task_group {
        async let pause = Timer.sleep(ms: 1)
        let done = await pause?
    }
    Log.done(client: read client)
    return Ok(Unit)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["await_sites"], 1);
    assert!(json["await_sites"].as_array().is_some_and(|await_sites| {
        await_sites.iter().any(|site| {
            site["function"] == "Api.run"
                && site["callee"] == "Timer.sleep"
                && site["boundary"] == "runtime_pending"
                && site["live_across_await"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value == "client"))
        })
    }));
    assert!(
        human.contains("Api.run awaits Timer.sleep (runtime_pending)"),
        "{human}"
    );
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "async_boundary"
                && fact["evidence"].as_array().is_some_and(|evidence| {
                    evidence.iter().any(|item| {
                        item["reason"].as_str().is_some_and(|reason| {
                            reason.contains("boundary=runtime_pending")
                                && reason.contains("callee=Timer.sleep")
                        })
                    })
                })
        })
    }));
}

#[test]
fn package_review_marks_async_native_await_boundary() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-async-native-boundary");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: async, native

struct HostError

pub async native fn Host.wait(ms: Int) -> Result<Unit, HostError>
    effects(native)

pub async fn Api.run() -> Result<Unit, HostError>
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: async

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
fn package_review_marks_rss_async_await_boundary() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-rss-async-boundary");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: async

struct AppError

pub async fn Work.step() -> Result<Unit, AppError>

pub async fn Api.run() -> Result<Unit, AppError>
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: async

pub async fn Work.step() -> Result<Unit, AppError> {
    return Ok(Unit)
}

pub async fn Api.run() -> Result<Unit, AppError> {
    await Work.step()?
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
                && site["callee"] == "Work.step"
                && site["boundary"] == "rss_call"
        })
    }));
}

#[test]
fn package_review_explains_manifest_unknown_risk() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-manifest-unknown");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.expect]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "manifest declares unknown package risk")
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
        r#"features: native, unsafe

native fn Native.echo(message: read String) -> String
    effects(native)
fn Native.danger(message: read String) -> String
    effects(unsafe)
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
fn package_review_json_records_parallel_native_metadata() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-parallel-native");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_parallel_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"features: native

native fn Parallel.sort(values: mut List<Int>) -> Unit
    effects(native, parallel)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_parallel_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nrayon = \"1.12\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "use rayon::prelude::*;\npub fn sort(values: &mut Vec<i64>) { values.par_sort_unstable(); }\n",
    )
    .expect("native source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["native_apis"], 1);
    assert_eq!(json["summary"]["parallel_apis"], 1);
    assert_eq!(
        json["native_rust"]["semantic"]["author_declaration"]["worker_thread_parallelism"],
        true
    );
    assert_eq!(
        json["native_rust"]["semantic"]["author_declaration"]["native_parallel_backend"],
        "rayon"
    );
    assert_eq!(
        json["native_rust"]["semantic"]["source_scan_best_effort"]["worker_thread_parallelism_detected"],
        true
    );
    assert!(
        json["native_rust"]["semantic"]["source_scan_best_effort"]["native_parallel_backends"]
            .as_array()
            .is_some_and(|backends| backends.iter().any(|backend| backend == "rayon"))
    );
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["capability"]["category"] == "process.spawn"
                && fact["capability"]["service"] == "native_rust_author_declaration"
                && fact["confidence"]["level"] == "declared"
                && fact["acquisition_mode"] == "manual_declaration"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "native_unsafe_slice")
    }));
}

#[test]
fn package_review_reir_maps_process_facade_to_process_capability() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-process-facade-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-process-facade",
        "0.1.0",
        "",
        r#"features: native

pub native fn Process.run_stdout(
    command: read String,
    args: read List<String>,
) -> Result<String, String>
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"]
                    == "rss-process-facade::public::function::Process.run_stdout"
                && fact["capability"]["category"] == "process.spawn"
                && fact["capability"]["service"] == "stdlib"
                && fact["evidence"][0]["kind"] == "package_metadata"
        })
    }));
    assert!(
        reir_json["slices"]
            .as_array()
            .is_some_and(|slices| { slices.iter().any(|slice| slice["kind"] == "process_slice") })
    );
}

#[test]
fn package_review_reir_finds_missing_mock_iam_permission_for_bound_capability() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-s3-capability-reir");
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

[[review.capability_bindings]]
symbol = "S3.put_object"
category = "object_storage.write"
provider = "aws"
service = "s3"
action = "s3:PutObject"
resource = "arn:aws:s3:::reports-prod/*"
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/s3.rssi"),
        r#"features: native

native fn S3.put_object(body: read String) -> Result<Unit, String>
    effects(native)
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
    let bundle: reir::Bundle =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR bundle should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    let required = bundle
        .facts
        .iter()
        .find(|fact| {
            fact.kind == reir::FactKind::Capability
                && fact.role == Some(reir::FactRole::Required)
                && fact.subject.id == "rss-report-upload::function::upload_report"
                && fact
                    .capability
                    .as_ref()
                    .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
        })
        .expect("upload_report should require s3:PutObject through the S3 binding");
    assert!(required.evidence.iter().any(|evidence| {
        evidence.kind == reir::EvidenceKind::BindingManifest
            && evidence.file.as_deref() == Some("src/upload.rss")
            && evidence.line == Some(2)
            && evidence
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("upload_report -> S3.put_object"))
    }));

    let required_facts = bundle
        .facts
        .iter()
        .filter(|fact| fact.role == Some(reir::FactRole::Required))
        .cloned()
        .collect::<Vec<_>>();
    let granted_facts = vec![mock_iam_grant("s3:GetObject")];
    let reconciliations =
        reir::reconcile_capabilities_for_target(&required_facts, &granted_facts, Some("prod"));

    assert!(reconciliations.iter().any(|reconciliation| {
        reconciliation.kind == reir::ReconciliationKind::MissingCapability
            && reconciliation.target.as_deref() == Some("prod")
            && reconciliation
                .capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
            && reconciliation.required_fact.as_ref() == Some(&required.id)
    }));
}

#[test]
fn package_review_reir_marks_unbound_native_facade_capability_unknown() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-unbound-native-capability");
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
        r#"features: native

native fn S3.put_object(body: read String) -> Result<Unit, String>
    effects(native)
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
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR bundle should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_ne!(review.risk, rsscript::PackageRisk::Unknown);
    assert_eq!(review_json["summary"]["unknown_apis"], 1);
    assert!(review_json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "native/external capability binding unknown")
    }));
    assert!(
        review_json["capabilities"]
            .as_array()
            .is_some_and(|capabilities| {
                capabilities.iter().any(|capability| {
                    capability["function"] == "upload_report"
                        && capability["binding_symbol"] == "S3.put_object"
                        && capability["category"] == "unknown"
                        && capability["unknown_reason"]
                            == "native/external facade has no review.capability_bindings entry"
                        && capability["call_chain"].as_array().is_some_and(|chain| {
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
                == Some("native/external facade has no review.capability_bindings entry")
            && fact.capability.as_ref().is_some_and(|capability| {
                capability.category == reir::CapabilityCategory::Unknown
                    && capability.action.as_deref() == Some("S3.put_object")
            })
    }));
}

#[test]
fn s3_iam_reir_demo_preserves_call_site_for_missing_permission() {
    let demo_dir = Path::new("demos/s3-iam-reir");
    let review = review_package_dir(demo_dir).expect("demo package review should succeed");
    let bundle: reir::Bundle =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("demo package REIR bundle should parse");

    let required = bundle
        .facts
        .iter()
        .find(|fact| {
            fact.kind == reir::FactKind::Capability
                && fact.role == Some(reir::FactRole::Required)
                && fact.subject.id == "rss-s3-uploader::function::upload_report"
                && fact
                    .capability
                    .as_ref()
                    .is_some_and(|capability| capability.action.as_deref() == Some("s3:PutObject"))
        })
        .expect("demo upload_report should require s3:PutObject");

    assert!(bundle.facts.iter().any(|fact| {
        fact.kind == reir::FactKind::NativeBoundary
            && fact.subject.id == "rss-s3-uploader::S3.put_object"
    }));
    assert!(!bundle.facts.iter().any(|fact| {
        fact.kind == reir::FactKind::NativeBoundary
            && fact.subject.id == "rss-s3-uploader::upload_report"
    }));
    assert!(required.evidence.iter().any(|evidence| {
        evidence.kind == reir::EvidenceKind::BindingManifest
            && evidence.file.as_deref() == Some("src/upload.rss")
            && evidence.line == Some(8)
            && evidence
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("upload_report -> S3.put_object"))
    }));
    assert_eq!(review.summary.await_sites, 8);
}

#[test]
fn s3_iam_reir_demo_lowers_native_s3_binding_to_runtime_tokio_pending() {
    let demo_dir = Path::new("demos/s3-iam-reir");
    let input = package_lowering_input(demo_dir).expect("demo package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("demo package source should lower");

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(
        input.native_dependencies[0].crate_name,
        "rss_s3_demo_native"
    );
    assert!(
        input.native_dependencies[0]
            .bindings
            .iter()
            .any(|(symbol, target)| {
                symbol == "S3.put_object" && target == "rss_s3_demo_native::put_object_start"
            })
    );
    assert!(
        package
            .lib_rs
            .contains("run_pending(rss_s3_demo_native::put_object_start"),
        "async native S3 call should lower through RSScript Pending runtime:\n{}",
        package.lib_rs
    );
    assert!(
        package.lib_rs.contains(
            "__rsscript_async_executor.run_pending(rss_s3_demo_native::put_object_start(&bucket, &key, &body))?;"
        ),
        "direct await of native S3 call should also poll the Pending runtime:\n{}",
        package.lib_rs
    );
    assert!(
        package
            .cargo_toml
            .contains("\"rss_s3_demo_native\" = { path = "),
        "generated package should depend on native S3 wrapper"
    );
}

#[test]
fn package_review_reir_maps_args_facade_to_process_args_capability() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-args-facade-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-args-facade",
        "0.1.0",
        "",
        r#"features: native

pub native fn Args.count() -> Int
    effects(native)

pub native fn Args.get_or_default(
    index: Int,
    default: read String,
) -> String
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        ["Args.count", "Args.get_or_default"].iter().all(|name| {
            facts.iter().any(|fact| {
                fact["kind"] == "capability"
                    && fact["subject"]["id"].as_str().is_some_and(|id| {
                        id == format!("rss-args-facade::public::function::{name}")
                    })
                    && fact["capability"]["category"] == "process.args"
                    && fact["capability"]["service"] == "stdlib"
                    && fact["evidence"][0]["kind"] == "package_metadata"
            })
        })
    }));
    assert!(
        reir_json["slices"]
            .as_array()
            .is_some_and(|slices| { slices.iter().any(|slice| slice["kind"] == "process_slice") })
    );
}

#[test]
fn package_review_reir_maps_random_facade_to_random_capability() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-random-facade-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-random-facade",
        "0.1.0",
        "",
        r#"features: native

pub native fn Uuid.new_v4() -> fresh String
    effects(native)

pub native fn Random.bytes(len: Int) -> fresh Bytes
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"] == "rss-random-facade::public::function::Random.bytes"
                && fact["capability"]["category"] == "random.read"
                && fact["capability"]["service"] == "stdlib"
                && fact["evidence"][0]["kind"] == "package_metadata"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"] == "rss-random-facade::public::function::Uuid.new_v4"
                && fact["capability"]["category"] == "random.read"
                && fact["capability"]["service"] == "stdlib"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "randomness_slice")
    }));
}

#[test]
fn package_review_reir_maps_env_http_time_hash_regex_and_tempdir_facades() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-stdlib-facades-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-stdlib-facades",
        "0.1.0",
        "",
        r#"features: native, local

resource TempDir
struct Instant
struct Regex
struct RegexError

pub native fn Env.get(name: read String) -> Option<fresh String>
    effects(native)

pub native fn Env.set(name: read String, value: read String) -> Unit
    effects(native)

pub native fn Http.post_json(url: read Url, body: read String) -> Result<fresh HttpResponse, HttpError>
    effects(native)

pub native fn Clock.now() -> fresh Instant
    effects(native)

pub native fn Hash.sha256_string(value: read String) -> fresh String
    effects(native)

pub native fn Hash.sha256_file(path: read Path) -> Result<fresh String, FileError>
    effects(native)

pub native fn Regex.compile(pattern: read String) -> Result<fresh Regex, RegexError>
    effects(native)

pub native fn TempDir.new() -> Result<TempDir, FileError>
    effects(native)

pub native fn TempDir.path(dir: read TempDir) -> fresh Path
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    let facts = reir_json["facts"]
        .as_array()
        .expect("REIR facts should be an array");
    for (name, category) in [
        ("Env.get", "env.read"),
        ("Env.set", "env.write"),
        ("Http.post_json", "network.client"),
        ("Clock.now", "time.read"),
        ("Hash.sha256_string", "compute.hash"),
        ("Hash.sha256_file", "compute.hash"),
        ("Hash.sha256_file", "filesystem.read"),
        ("Regex.compile", "compute.regex"),
        ("TempDir.new", "filesystem.write"),
        ("TempDir.path", "filesystem.read"),
    ] {
        assert!(
            facts.iter().any(|fact| {
                fact["kind"] == "capability"
                    && fact["subject"]["id"].as_str().is_some_and(|id| {
                        id == format!("rss-stdlib-facades::public::function::{name}")
                    })
                    && fact["capability"]["category"] == category
                    && fact["capability"]["service"] == "stdlib"
                    && fact["evidence"][0]["kind"] == "package_metadata"
            }),
            "missing stdlib capability fact for {name} -> {category}: {facts:?}"
        );
    }

    let slices = reir_json["slices"]
        .as_array()
        .expect("REIR slices should be an array");
    for kind in [
        "env_slice",
        "network_slice",
        "time_slice",
        "compute_slice",
        "filesystem_slice",
    ] {
        assert!(
            slices.iter().any(|slice| slice["kind"] == kind),
            "missing REIR slice {kind}: {slices:?}"
        );
    }
}

#[test]
fn package_review_reir_maps_log_facade_to_telemetry_capability() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-log-facade-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-log-facade",
        "0.1.0",
        "",
        r#"features: native

pub fn Log.write(message: read String) -> Unit
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"] == "rss-log-facade::public::function::Log.write"
                && fact["capability"]["category"] == "telemetry.emit"
                && fact["capability"]["service"] == "stdlib"
                && fact["evidence"][0]["kind"] == "package_metadata"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "telemetry_slice")
    }));
}

#[test]
fn package_review_reir_does_not_map_os_close_to_external_capability() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-os-close-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-os-close",
        "0.1.0",
        "",
        r#"features: native

pub native fn OS.close(fd: Fd) -> Unit
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    let facts = reir_json["facts"]
        .as_array()
        .expect("REIR facts should be an array");
    assert!(
        !facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"] == "rss-os-close::public::function::OS.close"
        }),
        "OS.close should remain native/resource cleanup evidence, not an external capability fact: {facts:?}"
    );
}

#[test]
fn package_review_reir_maps_csv_and_config_facades_to_filesystem_read() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-data-file-facades-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-data-file-facades",
        "0.1.0",
        "",
        r#"pub fn Csv.open_read(path: read Path) -> Result<File, CsvError>

pub fn Csv.read_into(
    file: mut File,
    buffer: mut RowBuffer,
) -> Result<Unit, CsvError>

pub fn Config.load(path: read Path) -> Result<fresh ConfigValue, ConfigError>

pub fn RuleLoader.load_rules(path: read Path) -> Result<fresh List<Rule>, ConfigError>
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        [
            "Csv.open_read",
            "Csv.read_into",
            "Config.load",
            "RuleLoader.load_rules",
        ]
        .iter()
        .all(|name| {
            facts.iter().any(|fact| {
                fact["kind"] == "capability"
                    && fact["subject"]["id"].as_str().is_some_and(|id| {
                        id == format!("rss-data-file-facades::public::function::{name}")
                    })
                    && fact["capability"]["category"] == "filesystem.read"
                    && fact["capability"]["service"] == "stdlib"
                    && fact["evidence"][0]["kind"] == "package_metadata"
            })
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "filesystem_slice")
    }));
}

#[test]
fn package_review_reir_maps_file_json_and_toml_facades_to_filesystem_capabilities() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-file-json-toml-facades-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-file-json-toml-facades",
        "0.1.0",
        "",
        r#"features: native

resource File

pub fn File.open(path: read Path) -> Result<File, FileError>

pub fn File.read_all_string(file: mut File) -> Result<String, FileError>

pub fn File.write_buffer(file: mut File, buffer: read Buffer) -> Result<Unit, FileError>

pub fn Json.parse_file(path: read Path) -> Result<fresh JsonValue, JsonError>

pub native fn Toml.parse_file(path: read Path) -> Result<fresh JsonValue, JsonError>
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        [
            "File.open",
            "File.read_all_string",
            "Json.parse_file",
            "Toml.parse_file",
        ]
        .iter()
        .all(|name| {
            facts.iter().any(|fact| {
                fact["kind"] == "capability"
                    && fact["subject"]["id"].as_str().is_some_and(|id| {
                        id == format!("rss-file-json-toml-facades::public::function::{name}")
                    })
                    && fact["capability"]["category"] == "filesystem.read"
                    && fact["capability"]["service"] == "stdlib"
            })
        }) && ["File.open", "File.write_buffer"].iter().all(|name| {
            facts.iter().any(|fact| {
                fact["kind"] == "capability"
                    && fact["subject"]["id"].as_str().is_some_and(|id| {
                        id == format!("rss-file-json-toml-facades::public::function::{name}")
                    })
                    && fact["capability"]["category"] == "filesystem.write"
                    && fact["capability"]["service"] == "stdlib"
            })
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "filesystem_slice")
    }));
}

#[test]
fn package_review_reir_maps_db_and_image_facades_to_capabilities() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-db-image-facades-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-db-image-facades",
        "0.1.0",
        "",
        r#"resource DbConnection

pub fn DbConnection.open(url: read Url) -> DbConnection

pub fn DbConnection.query(
    conn: mut DbConnection,
    sql: read String,
) -> Result<Unit, DbError>

pub fn Image.load(path: read Path) -> Result<fresh Image, ImageError>

pub fn Image.save(image: read Image, path: read Path) -> Result<Unit, ImageError>
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"]
                    == "rss-db-image-facades::public::function::DbConnection.query"
                && fact["capability"]["category"] == "database.read"
                && fact["capability"]["service"] == "stdlib"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"]
                    == "rss-db-image-facades::public::function::DbConnection.query"
                && fact["capability"]["category"] == "database.write"
                && fact["capability"]["service"] == "stdlib"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"] == "rss-db-image-facades::public::function::Image.load"
                && fact["capability"]["category"] == "filesystem.read"
                && fact["capability"]["service"] == "stdlib"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "capability"
                && fact["subject"]["id"] == "rss-db-image-facades::public::function::Image.save"
                && fact["capability"]["category"] == "filesystem.write"
                && fact["capability"]["service"] == "stdlib"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices.iter().any(|slice| slice["kind"] == "database_slice")
            && slices
                .iter()
                .any(|slice| slice["kind"] == "filesystem_slice")
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
        r#"features: native

native fn Build.run() -> Unit
    effects(native)
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
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
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
            fact["kind"] == "capability"
                && fact["capability"]["category"] == "build.execute"
                && fact["capability"]["service"] == "native_rust_source_scan"
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
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"

[native.rust.feature_map]
wasm-browser = { cargo_features = ["rayon/web_spin_lock"] }
"#,
        r#"features: native

native fn Feature.value() -> Int
    effects(native)
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
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
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
            .any(|feature| feature == "rayon/web_spin_lock")
    );
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "native_cargo_feature"
                && fact["subject"]["id"] == "rss-json@0.1.0#native-cargo-feature:base-native"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "native_cargo_feature"
                && fact["subject"]["id"]
                    == "rss-json@0.1.0#native-cargo-feature:rayon/web_spin_lock"
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
fn package_review_json_counts_public_api_review_categories() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-api-categories");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"struct Image
resource DbConnection

pub fn Image.load(path: read String) -> fresh Image
pub fn Cache.store(conn: mut DbConnection, image: read Image) -> Unit
    effects(retains(image))
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_types"], 2);
    assert_eq!(json["summary"]["public_functions"], 2);
    assert_eq!(json["summary"]["public_apis"], 4);
    assert_eq!(json["summary"]["mutating_apis"], 1);
    assert_eq!(json["summary"]["retaining_apis"], 1);
    assert_eq!(json["summary"]["resource_apis"], 1);
    assert_eq!(json["summary"]["fresh_returning_apis"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 0);
    let exports = json["exports"]
        .as_array()
        .expect("exports should be an array");
    assert!(exports.iter().any(|export| {
        export["name"] == "DbConnection"
            && export["kind"] == "type"
            && export["reasons"]
                .as_array()
                .is_some_and(|reasons| reasons.iter().any(|reason| reason == "resource type"))
    }));
    assert!(exports.iter().any(|export| {
        export["name"] == "Cache.store"
            && export["kind"] == "function"
            && export["classification"] == "review_if_changed"
            && export["reasons"].as_array().is_some_and(|reasons| {
                reasons
                    .iter()
                    .any(|reason| reason == "mut parameter `conn`")
                    && reasons.iter().any(|reason| reason == "retains(image)")
                    && reasons
                        .iter()
                        .any(|reason| reason == "resource parameter `conn`")
            })
    }));
    assert!(human.contains("exports:"));
    assert!(human.contains("function Cache.store: review_if_changed"));
    assert!(human.contains("retains(image)"));
    assert!(human.contains("type DbConnection: review_if_changed"));
}

#[test]
fn package_review_exports_protocol_impl_contracts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-protocol-impl-export");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-protocol-export",
        "0.1.0",
        "",
        r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

pub fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

impl Writer for BufferWriter {
    write = BufferWriter.write
}
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Writer for BufferWriter"
                && export["kind"] == "protocol_impl"
                && export["classification"] == "review_if_changed"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "write = BufferWriter.write")
                })
        })
    }));
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Writer"
                && export["kind"] == "protocol"
                && export["classification"] == "review_if_changed"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons.iter().any(|reason| reason == "method `write`")
                        && reasons.iter().any(|reason| {
                            reason
                                == "method contract `fn Writer.write(self: mut Self, message: read String) -> Unit effects(retains(message))`"
                        })
                })
        })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "protocol_declaration"
                && fact["subject"]["kind"] == "code.protocol"
                && fact["subject"]["id"] == "rss-protocol-export::public::protocol::Writer"
                && fact["value"] == true
        }) && facts.iter().any(|fact| {
            fact["kind"] == "protocol_method_contract"
                && fact["subject"]["kind"] == "code.protocol_method"
                && fact["subject"]["id"] == "rss-protocol-export::protocol::Writer::method::write"
                && fact["value"] == true
        })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "protocol_impl"
                && fact["subject"]["kind"] == "code.protocol_impl"
                && fact["subject"]["id"]
                    == "rss-protocol-export::public::protocol_impl::Writer for BufferWriter"
                && fact["value"] == true
        })
    }));
    assert!(reir_json["edges"].as_array().is_some_and(|edges| {
        edges.iter().any(|edge| {
            edge["kind"] == "implements_protocol"
                && edge["from"]["id"]
                    == "rss-protocol-export::public::protocol_impl::Writer for BufferWriter"
                && edge["to"]["id"] == "rss-protocol-export::public::protocol::Writer"
        })
    }));
}

#[test]
fn package_review_reports_protocol_impl_contract_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-protocol-impl-mismatch");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-protocol-mismatch",
        "0.1.0",
        "",
        r#"protocol Writer {
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
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

pub fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))
{
    Log.write(message: read message)
}

pub fn BufferWriter.audit_write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))
{
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.audit_write
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(review.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1301"
            && diagnostic.label == "interface/source protocol implementation mismatch"
            && diagnostic
                .causes
                .iter()
                .any(|cause| cause.contains("impl Writer for BufferWriter"))
    }));
}

#[test]
fn package_review_reports_interface_protocol_contract_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-protocol-contract-mismatch");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-protocol-contract-mismatch",
        "0.1.0",
        "",
        r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(review.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1301"
            && diagnostic.label == "interface/source protocol mismatch"
            && diagnostic
                .causes
                .iter()
                .any(|cause| cause.contains("effects(retains(message))"))
    }));
}

#[test]
fn package_review_accepts_matching_interface_protocol_contract() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-protocol-contract-match");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-protocol-contract-match",
        "0.1.0",
        "",
        r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!review.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1301" && diagnostic.label == "interface/source protocol mismatch"
    }));
}

#[test]
fn package_review_source_visibility_excludes_private_types() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-type-visibility");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-visibility"
version = "0.1.0"
edition = "2026"

[sources]
paths = ["src"]
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub struct PublicConfig {
    name: String
}

struct PrivateConfig {
    name: String
}

pub fn load() -> fresh PublicConfig {
    return PublicConfig(name: "ok")
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_types"], 1);
    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["public_apis"], 2);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports
            .iter()
            .any(|export| export["name"] == "PublicConfig" && export["kind"] == "type")
            && !exports
                .iter()
                .any(|export| export["name"] == "PrivateConfig")
    }));
}

#[test]
fn package_review_exports_public_data_model_contracts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-data-model-exports");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-data-model"
version = "0.1.0"
edition = "2026"

[sources]
paths = ["src"]
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub sum PackageError {
    Io(path: String),
    Invalid
}

sum PrivateError {
    Hidden
}

pub type PackageName = String
type PrivateName = String

pub const MAX_RETRIES: Int = 3
const INTERNAL_RETRIES: Int = 1
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_sum_types"], 1);
    assert_eq!(json["summary"]["public_type_aliases"], 1);
    assert_eq!(json["summary"]["public_consts"], 1);
    assert_eq!(json["summary"]["public_apis"], 3);
    let exports = json["exports"]
        .as_array()
        .expect("exports should be an array");
    assert!(exports.iter().any(|export| {
        export["name"] == "PackageError"
            && export["kind"] == "sum_type"
            && export["reasons"].as_array().is_some_and(|reasons| {
                reasons.iter().any(|reason| reason == "public sum type")
                    && reasons.iter().any(|reason| reason == "variant `Io`")
            })
    }));
    assert!(exports.iter().any(|export| {
        export["name"] == "PackageName"
            && export["kind"] == "type_alias"
            && export["reasons"]
                .as_array()
                .is_some_and(|reasons| reasons.iter().any(|reason| reason == "target `String`"))
    }));
    assert!(exports.iter().any(|export| {
        export["name"] == "MAX_RETRIES"
            && export["kind"] == "const"
            && export["reasons"]
                .as_array()
                .is_some_and(|reasons| reasons.iter().any(|reason| reason == "type `Int`"))
    }));
    assert!(!exports.iter().any(|export| {
        matches!(
            export["name"].as_str(),
            Some("PrivateError" | "PrivateName" | "INTERNAL_RETRIES")
        )
    }));
}

#[test]
fn package_review_json_counts_public_apis_with_unknown_review_regions() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-unknown-api");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    helper()
    return Unit
}

fn helper() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 2);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "unknown review-map region")
                })
        })
    }));
    assert!(human.contains("function Api.run: unknown"));
    assert!(human.contains("unknown review-map region"));
}

#[test]
fn package_review_json_counts_public_api_with_direct_unknown_call() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-direct-unknown-api");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    Missing.call()
    return Unit
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 1);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "unknown review-map region")
                })
        })
    }));
}

#[test]
fn package_review_map_resolves_path_dependency_interfaces() {
    let dep_dir = common::unique_temp_dir("rsscript-package-review-map-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-review-dep",
        "0.1.0",
        "",
        r#"features: native

native fn Dep.echo(message: read String) -> String
    effects(native)
"#,
    );

    let root_dir = common::unique_temp_dir("rsscript-package-review-map-root");
    common::write_named_package_fixture(
        &root_dir,
        "rss-review-root",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-review-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn Api.run(message: read String) -> String
"#,
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"features: native

pub fn Api.run(message: read String) -> String {
    return Dep.echo(message: read message)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(json["summary"]["public_functions"], 1);
    assert_eq!(json["summary"]["unknown_apis"], 0);
    assert_eq!(json["review_map"]["summary"]["unknown"]["functions"], 0);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Api.run" && export["classification"] == "review_if_changed"
        })
    }));
    assert!(
        json["review_map"]["files"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|file| file["regions"].as_array().into_iter().flatten())
            .any(|region| {
                region["function"] == "Api.run"
                    && region["reasons"].as_array().is_some_and(|reasons| {
                        reasons
                            .iter()
                            .any(|reason| reason == "native call `Dep.echo`")
                    })
            })
    );
}

#[test]
fn package_check_fails_unknown_review_when_configured_as_error() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-unknown-is-error");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[sources]
paths = ["src"]

[review.expect]
risk = "unknown"

[review.policy]
deny_unknown = true
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"pub fn Api.run() -> Unit {
    return Unit
}
"#,
    )
    .expect("source should be written");
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
    assert_eq!(json["lock"]["matches"], true);
    assert_eq!(json["summary"]["errors"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package policy denies unknown review risk")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "deny_unknown"
        })
    }));
}

#[test]
fn package_manifest_rejects_legacy_review_policy_aliases() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-policy-alias");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review]
unknown_is_error = true
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let error =
        review_package_dir(&temp_dir).expect_err("legacy review aliases should be rejected");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(error.contains("unknown field"), "{error}");
    assert!(error.contains("unknown_is_error"), "{error}");
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
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
    assert_eq!(json["risk"], "high");
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
fn package_check_fails_when_policy_denies_unsafe_api() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-deny-unsafe");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
deny_unsafe_apis = true
"#,
        r#"features: unsafe

fn Native.danger(message: read String) -> String
    effects(unsafe)
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
    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package policy denies unsafe public APIs")
    }));
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "deny_unsafe_apis"
        })
    }));
}

#[test]
fn package_check_applies_public_signature_policy_limits() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-signature-policy");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
max_public_params = 2
max_nested_type_depth = 2
"#,
        r#"struct Error

pub fn Api.run(
    first: read String,
    second: read String,
    third: read String,
) -> Result<List<String>, Error>
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
    assert_eq!(json["risk"], "high");
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "max_public_params"
        }) && diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "max_nested_type_depth"
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "elevated");
    assert_eq!(json["summary"]["native_apis"], 1);
}

#[test]
fn package_check_reports_invalid_review_policy_values() {
    let temp_dir = common::unique_temp_dir("rsscript-package-invalid-review-policy");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[review.policy]
native_api_risk = "low"
build_execution_default = "sometimes"
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
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "native_api_risk"
        }) && diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0501" && diagnostic["label"] == "build_execution_default"
        })
    }));
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
    assert_eq!(json["summary"]["errors"], 5);
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["native_rust"]["build_scripts"], "review");
    assert_eq!(json["native_rust"]["proc_macros"], "forbid");
    assert_eq!(json["native_rust"]["unsafe_policy"], "review");
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
        r#"features: native

native fn Native.value() -> Int
    effects(native)
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
fn package_review_marks_broken_rssi_contract_diagnostics_unknown() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-broken-rssi");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"fn (value: read String) -> Unit
"#,
    );

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_review_reir_json(&review))
            .expect("package review REIR JSON should parse");
    let human = rsscript::format_package_review_human(&review);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["summary"]["unknown_apis"], 1);
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "public .rssi contract contains frontend errors")
    }));
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["kind"] == "contract_diagnostic"
                && export["classification"] == "unknown"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "frontend error RS0015")
                })
        })
    }));
    assert!(human.contains("contract_diagnostic interface/lib.rssi:1:1: unknown"));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "diagnostic"
                && fact["value"] == "unknown"
                && fact["evidence"].as_array().is_some_and(|evidence| {
                    evidence.iter().any(|item| {
                        item["symbol"] == "RS0015"
                            && item["file"]
                                .as_str()
                                .is_some_and(|file| file.ends_with("interface/lib.rssi"))
                    })
                })
        })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "public_contract"
                && fact["value"] == "unknown"
                && fact["confidence"]["level"] == "unknown"
                && fact["subject"]["id"].as_str().is_some_and(|id| {
                    id.contains("public::contract_diagnostic::interface/lib.rssi:1:1")
                })
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "diagnostic_slice")
    }));
}

#[test]
fn package_metadata_dry_run_reports_review_metadata_without_writing() {
    let temp_dir = common::unique_temp_dir("rsscript-package-metadata-dry-run");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-metadata",
        "0.1.0",
        r#"[features]
fast = []
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let metadata = package_metadata(&temp_dir, true).expect("metadata dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&metadata))
        .expect("metadata JSON should parse");
    let metadata_path_exists = temp_dir.join("review").join("package-review.json").exists();
    let reir_path_exists = temp_dir
        .join("review")
        .join("reir")
        .join("rsscript.json")
        .exists();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(metadata.ok);
    assert!(!metadata_path_exists);
    assert!(!reir_path_exists);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["written"], false);
    assert_eq!(json["verified"], false);
    assert_eq!(json["mismatches"], serde_json::json!([]));
    assert!(
        json["metadata_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("review/package-review.json"))
    );
    assert!(
        json["reir_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("review/reir/rsscript.json"))
    );
    assert_eq!(json["metadata"]["schema"], "rss.review.package.v1");
    assert_eq!(json["metadata"]["package"]["name"], "rss-metadata");
    assert_eq!(json["metadata"]["features"], serde_json::json!(["fast"]));
}

#[test]
fn package_metadata_writes_reir_review_artifact() {
    let temp_dir = common::unique_temp_dir("rsscript-package-metadata-reir");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-metadata-reir",
        "0.1.0",
        "",
        r#"features: native

pub fn NativeBridge.run(value: read Int) -> Int
    effects(native)
"#,
    );

    let metadata = package_metadata(&temp_dir, false).expect("metadata write should succeed");
    let package_review_json =
        fs::read_to_string(temp_dir.join("review").join("package-review.json"))
            .expect("package review metadata should be written");
    let reir_json = fs::read_to_string(temp_dir.join("review").join("reir").join("rsscript.json"))
        .expect("REIR metadata should be written");
    let package_review: Value =
        serde_json::from_str(&package_review_json).expect("package review should parse");
    let reir: Value = serde_json::from_str(&reir_json).expect("REIR bundle should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(metadata.written);
    assert!(!metadata.verified);
    assert_eq!(package_review["package"]["name"], "rss-metadata-reir");
    assert_eq!(reir["schema"], "reir.bundle.v0.1");
    assert!(reir["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| fact["kind"] == "package_risk")
            && facts.iter().any(|fact| fact["kind"] == "native_boundary")
    }));
}

#[test]
fn package_metadata_verify_accepts_current_review_artifacts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-metadata-verify-current");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-metadata-verify-current",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    package_metadata(&temp_dir, false).expect("metadata write should succeed");
    let verified =
        package_metadata_verify(&temp_dir).expect("metadata verify should recompute package");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&verified))
        .expect("verified metadata JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(verified.ok);
    assert!(verified.verified);
    assert!(!verified.written);
    assert_eq!(json["verified"], true);
    assert_eq!(json["mismatches"], serde_json::json!([]));
}

#[test]
fn package_metadata_verify_reports_missing_or_stale_artifacts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-metadata-verify-stale");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-metadata-verify-stale",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    package_metadata(&temp_dir, false).expect("metadata write should succeed");
    fs::write(
        temp_dir.join("review").join("package-review.json"),
        "{\"schema\":\"rss.review.package.v1\",\"stale\":true}",
    )
    .expect("package review artifact should be made stale");
    fs::remove_file(temp_dir.join("review").join("reir").join("rsscript.json"))
        .expect("REIR artifact should be removed");

    let verified =
        package_metadata_verify(&temp_dir).expect("metadata verify should report mismatches");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_metadata_reir_json(&verified))
            .expect("metadata REIR JSON should parse");
    let mismatch_kinds = verified
        .mismatches
        .iter()
        .map(|mismatch| mismatch.kind.as_str())
        .collect::<Vec<_>>();
    let stale_mismatch = verified
        .mismatches
        .iter()
        .find(|mismatch| mismatch.kind == "stale")
        .expect("stale package review metadata mismatch should be reported");
    let missing_mismatch = verified
        .mismatches
        .iter()
        .find(|mismatch| mismatch.kind == "missing")
        .expect("missing REIR metadata mismatch should be reported");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!verified.ok);
    assert!(!verified.verified);
    assert!(!verified.written);
    assert!(mismatch_kinds.contains(&"stale"));
    assert!(mismatch_kinds.contains(&"missing"));
    assert_eq!(stale_mismatch.artifact, "package_review");
    assert!(stale_mismatch.expected_sha256.starts_with("sha256:"));
    assert!(
        stale_mismatch
            .actual_sha256
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert_eq!(missing_mismatch.artifact, "reir_bundle");
    assert!(missing_mismatch.expected_sha256.starts_with("sha256:"));
    assert_eq!(missing_mismatch.actual_sha256, None);
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "supply_chain"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with(".reir_artifact"))
                && fact["value"] == "unknown"
                && fact["evidence"][0]["kind"] == "package_metadata"
                && fact["evidence"][0]["json_pointer"] == "/reir_path"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.contains(".mismatch."))
                && fact["evidence"][0]["json_pointer"]
                    .as_str()
                    .is_some_and(|pointer| pointer.starts_with("/mismatches/"))
        })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"].as_str().is_some_and(|id| {
                    id.contains(".mismatch.") && id.contains("review_package_review_json")
                })
                && fact["evidence"][0]["value"].as_str().is_some_and(|value| {
                    value.contains("expected=sha256:") && value.contains("actual=sha256:")
                })
                && fact["evidence"][0]["file"] == stale_mismatch.path
                && fact["evidence"][0]["reason"]
                    .as_str()
                    .is_some_and(|reason| {
                        reason.contains("metadata package_review stale")
                            && reason.contains("expected sha256:")
                            && reason.contains("actual sha256:")
                    })
                && fact["unknown_reason"].as_str().is_some_and(|reason| {
                    reason.contains("metadata artifact")
                        && reason.contains("stale")
                        && reason.contains("expected sha256:")
                        && reason.contains("actual sha256:")
                })
        }) && facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.contains(".mismatch.") && id.contains("rsscript_json"))
                && fact["evidence"][0]["value"].as_str().is_some_and(|value| {
                    value.contains("review/reir/rsscript.json")
                        && value.contains("expected=sha256:")
                        && !value.contains("actual=sha256:")
                })
                && fact["evidence"][0]["file"] == missing_mismatch.path
                && fact["evidence"][0]["reason"]
                    .as_str()
                    .is_some_and(|reason| {
                        reason.contains("metadata reir_bundle missing")
                            && reason.contains("expected sha256:")
                            && !reason.contains("actual sha256:")
                    })
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices.iter().any(|slice| {
            slice["kind"] == "package_risk_slice"
                && slice["facts"].as_array().is_some_and(|facts| {
                    facts.iter().any(|fact| {
                        fact.as_str()
                            .is_some_and(|id| id.ends_with(".reir_artifact"))
                    })
                })
        })
    }));
}

#[test]
fn package_publish_archive_excludes_generated_review_artifacts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-publish-excludes-review-artifacts");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-publish-review-artifacts",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    package_metadata(&temp_dir, false).expect("metadata write should succeed");
    let publish_before_ci_artifacts =
        publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    fs::write(
        temp_dir
            .join("review")
            .join("reir")
            .join("rsscript-check.json"),
        "{\"schema\":\"reir.bundle.v0.1\",\"producer\":\"check\"}",
    )
    .expect("additional REIR CI artifact should be written");
    fs::create_dir_all(temp_dir.join("review").join("reir").join("ci"))
        .expect("nested REIR artifact directory should be created");
    fs::write(
        temp_dir
            .join("review")
            .join("reir")
            .join("ci")
            .join("rsscript-metadata-verify.json"),
        "{\"schema\":\"reir.bundle.v0.1\",\"producer\":\"metadata\"}",
    )
    .expect("nested REIR CI artifact should be written");
    let publish = publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_publish_reir_json(&publish))
            .expect("package publish REIR JSON should parse");
    let archive_paths = publish
        .archive_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(
        publish.archive_hash,
        publish_before_ci_artifacts.archive_hash
    );
    assert!(!archive_paths.contains(&"review/package-review.json"));
    assert!(!archive_paths.contains(&"review/reir/rsscript.json"));
    assert!(!archive_paths.contains(&"review/reir/rsscript-check.json"));
    assert!(!archive_paths.contains(&"review/reir/ci/rsscript-metadata-verify.json"));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"].as_str() == Some("supply_chain")
                && fact["id"].as_str()
                    == Some(
                        "fact.publish.rss_publish_review_artifacts_0_1_0.effective_interface_hash",
                    )
                && fact["evidence"][0]["kind"].as_str() == Some("registry_metadata")
                && fact["evidence"][0]["json_pointer"].as_str()
                    == Some("/registry_index/effective_interface_hash_default")
        }) && facts.iter().any(|fact| {
            fact["kind"].as_str() == Some("policy_result")
                && fact["id"].as_str()
                    == Some("fact.publish.rss_publish_review_artifacts_0_1_0.readiness")
                && fact["evidence"][0]["kind"].as_str() == Some("registry_metadata")
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "package_risk_slice")
    }));
}

#[test]
fn package_publish_registry_index_exposes_review_schema_features_and_footprint() {
    let temp_dir = common::unique_temp_dir("rsscript-package-publish-registry-index");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-publish-index",
        "0.1.0",
        r#"[features]
streaming = []

[dependencies]
rss-core = "0.5"
"#,
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );

    let publish = publish_package_dry_run(&temp_dir).expect("publish dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_publish_json(&publish))
        .expect("package publish JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_publish_reir_json(&publish))
            .expect("package publish REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(json["registry_index"]["schema"], "rss.registry.index.v1");
    assert_eq!(
        json["registry_index"]["review_schema"],
        "rss.review.package.v1"
    );
    assert_eq!(
        json["registry_index"]["effective_interface_hash_default"],
        json["registry_index"]["interface_hash"]
    );
    assert_eq!(
        json["registry_index"]["features"],
        serde_json::json!({
            "default": ["streaming"],
            "streaming": []
        })
    );
    assert_eq!(json["registry_index"]["unsafe_apis"], false);
    assert_eq!(
        json["registry_index"]["footprint_default"]["total_packages"],
        2
    );
    assert_eq!(
        json["registry_index"]["footprint_default"]["direct_dependencies"],
        1
    );
    assert_eq!(
        json["registry_index"]["footprint_default"]["unknown_facts"],
        1
    );
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["id"].as_str() == Some("fact.publish.rss_publish_index_0_1_0.readiness")
                && fact["evidence"][0]["kind"].as_str() == Some("registry_metadata")
        }) && facts.iter().any(|fact| {
            fact["id"].as_str() == Some("fact.publish.rss_publish_index_0_1_0.review_schema")
                && fact["kind"].as_str() == Some("supply_chain")
                && fact["evidence"][0]["json_pointer"].as_str()
                    == Some("/registry_index/review_schema")
        }) && facts.iter().any(|fact| {
            fact["id"].as_str() == Some("fact.publish.rss_publish_index_0_1_0.default_features")
                && fact["kind"].as_str() == Some("supply_chain")
                && fact["evidence"][0]["json_pointer"].as_str()
                    == Some("/registry_index/features/default")
        }) && facts.iter().any(|fact| {
            fact["id"].as_str() == Some("fact.publish.rss_publish_index_0_1_0.registry_footprint")
                && fact["kind"].as_str() == Some("dependency_risk")
                && fact["evidence"][0]["json_pointer"].as_str()
                    == Some("/registry_index/footprint_default")
                && fact["unknown_reason"].as_str()
                    == Some("registry preview footprint contains unknown or unresolved facts")
        }) && facts.iter().any(|fact| {
            fact["id"].as_str() == Some("fact.publish.rss_publish_index_0_1_0.registry_native")
                && fact["kind"].as_str() == Some("native_boundary")
                && fact["value"] == false
                && fact["evidence"][0]["json_pointer"].as_str() == Some("/registry_index/native")
        }) && facts.iter().any(|fact| {
            fact["id"].as_str() == Some("fact.publish.rss_publish_index_0_1_0.registry_unsafe_apis")
                && fact["kind"].as_str() == Some("unsafe_boundary")
                && fact["value"] == false
                && fact["evidence"][0]["json_pointer"].as_str()
                    == Some("/registry_index/unsafe_apis")
        })
    }));
}

#[test]
fn package_metadata_reports_unknown_review_risk_not_ok() {
    let temp_dir = common::unique_temp_dir("rsscript-package-metadata-unknown-risk");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-metadata-unknown",
        "0.1.0",
        r#"[review.expect]
risk = "unknown"
"#,
        r#"pub fn Api.run() -> Unit
"#,
    );

    let metadata = package_metadata(&temp_dir, true).expect("metadata dry-run should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_metadata_json(&metadata))
        .expect("metadata JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!metadata.ok);
    assert_eq!(json["ok"], false);
    assert_eq!(json["risk"], "unknown");
    assert_eq!(json["metadata"]["summary"]["errors"], 0);
}

#[test]
fn docs_do_not_reintroduce_legacy_gc_runtime_surface() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let legacy_runtime_name = ["runtime ", "G", "c"].concat();
    let legacy_runtime_path = ["rsscript_runtime::", "G", "c"].concat();
    let legacy_review_category = ["safe", "_to_", "skip"].concat();

    for relative_path in [
        "README.md",
        "RSScript_v0.6_Spec.md",
        "RSScript_Package_Manager_Design_v0.6.md",
    ] {
        let source = fs::read_to_string(root.join(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should read: {error}"));

        assert!(
            !source.contains(&legacy_runtime_name),
            "{relative_path} must describe managed runtime values as Managed<T>, not Gc"
        );
        assert!(
            !source.contains(&legacy_runtime_path),
            "{relative_path} must not expose legacy managed runtime aliases"
        );
        assert!(
            !source.contains(&legacy_review_category),
            "{relative_path} must emit low_semantic_risk instead of legacy review categories"
        );
    }
}

#[test]
fn package_manager_spec_uses_current_http_and_env_facade_shapes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = fs::read_to_string(root.join("RSScript_Package_Manager_Design_v0.6.md"))
        .expect("package manager spec should be readable");

    for stale in [
        "Http.HttpClient",
        "Http.Response",
        "Http.HttpError",
        "Http.Url",
        "Http.body_text",
        "Env.EnvError",
        "Result<String, Env.EnvError>",
    ] {
        assert!(
            !spec.contains(stale),
            "package manager spec should not reference stale facade shape `{stale}`"
        );
    }
    for current in [
        "pub native fn Http.get(\n    url: read Url,\n) -> Result<fresh HttpResponse, HttpError>",
        "pub native fn HttpResponse.text(\n    response: read HttpResponse,\n) -> fresh String",
        "pub native fn Env.get(name: read String) -> Option<fresh String>",
        "pub native fn Env.get_or_default(",
    ] {
        assert!(
            spec.contains(current),
            "package manager spec should document current facade shape `{current}`"
        );
    }
}

#[test]
fn package_manager_spec_uses_implemented_provider_resolution_manifest_shape() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = fs::read_to_string(root.join("RSScript_Package_Manager_Design_v0.6.md"))
        .expect("package manager spec should be readable");

    for stale in ["[provider]", "mode = \"platform_provided\""] {
        assert!(
            !spec.contains(stale),
            "package manager spec should not document unimplemented provider manifest shape `{stale}`"
        );
    }
    for current in [
        "platform-env = { path = \"../platform-env\", platform_provided = true }",
        "[providers]",
        "platform-env = { package = \"posix-env\", version = \"0.1.0\" }",
        "`[implements.\"<interface-package>\"]`",
        "`interface_effective_hash`",
    ] {
        assert!(
            spec.contains(current),
            "package manager spec should document implemented provider manifest shape `{current}`"
        );
    }
}

#[test]
fn reir_spec_keeps_os_close_as_descriptor_cleanup_not_external_capability() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = fs::read_to_string(root.join("Review_Evidence_IR_Spec_v0.2.md"))
        .expect("REIR spec should be readable");

    assert!(spec.contains("`OS.close`"));
    assert!(spec.contains("trusted native/resource"));
    assert!(spec.contains("do not imply `filesystem.read`, `filesystem.write`, or"));
}

#[test]
fn rss_spec_keeps_protocol_dynamic_dispatch_deferred() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = fs::read_to_string(root.join("RSScript_v0.6_Spec.md"))
        .unwrap_or_else(|error| panic!("RSScript spec should read: {error}"));

    for forbidden in [
        "Dynamic dispatch (admitted",
        "RSScript admits protocol-typed dynamic dispatch",
        "The design decision is settled: dynamic dispatch is supported",
        "form is admitted, not excluded",
        "protocol_dynamic_dispatch",
    ] {
        assert!(
            !spec.contains(forbidden),
            "v0.5 protocol dynamic dispatch must remain deferred, found `{forbidden}`"
        );
    }
    assert!(spec.contains("Dynamic dispatch (deferred, not admitted in v0.6)"));
    assert!(spec.contains("The only implemented and specified protocol call form is"));
    assert!(spec.contains("explicit `Protocol.method(...)` dispatch"));
}

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
    assert_eq!(json["risk"], "high");
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
fn package_lock_records_contract_review_and_native_hashes() {
    let temp_dir = common::unique_temp_dir("rsscript-package-lock");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_json_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/src/lib.rs"),
        "pub fn parse(_: &str) {}\n",
    )
    .expect("native source should be written");

    let lock = lock_package_dir(&temp_dir).expect("package lock should succeed");
    let toml = format_package_lock_toml(&lock);
    let _ = fs::remove_dir_all(&temp_dir);

    assert_eq!(lock.version, 1);
    assert_eq!(lock.packages.len(), 1);
    assert_eq!(lock.packages[0].name, "rss-json");
    assert_eq!(lock.packages[0].features, vec!["streaming".to_string()]);
    assert!(lock.packages[0].checksum.starts_with("sha256:"));
    assert!(lock.packages[0].interface_hash.starts_with("sha256:"));
    assert!(lock.packages[0].review_hash.starts_with("sha256:"));
    assert!(
        lock.packages[0]
            .native_hash
            .as_ref()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(toml.contains("[[package]]"));
    assert!(toml.contains("rss_version = \""));
    assert!(toml.contains("interface_hash = \"sha256:"));
}

#[test]
fn package_lock_review_hash_tracks_native_api_count_changes() {
    let old_dir = common::unique_temp_dir("rsscript-package-lock-native-api-old");
    let new_dir = common::unique_temp_dir("rsscript-package-lock-native-api-new");
    let native_manifest = r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#;
    common::write_package_fixture(
        &old_dir,
        "0.1.0",
        native_manifest,
        r#"features: native

native fn Native.one(message: read String) -> String
    effects(native)
"#,
    );
    common::write_package_fixture(
        &new_dir,
        "0.1.0",
        native_manifest,
        r#"features: native

native fn Native.one(message: read String) -> String
    effects(native)
native fn Native.two(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(old_dir.join("native")).expect("old native dir should be created");
    fs::write(
        old_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.one" = "rss_native::one"
"#,
    )
    .expect("old native bindings should be written");
    fs::create_dir_all(new_dir.join("native")).expect("new native dir should be created");
    fs::write(
        new_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.one" = "rss_native::one"
"Native.two" = "rss_native::two"
"#,
    )
    .expect("new native bindings should be written");

    let old_review = review_package_dir(&old_dir).expect("old package review should succeed");
    let new_review = review_package_dir(&new_dir).expect("new package review should succeed");
    let old_lock = lock_package_dir(&old_dir).expect("old package lock should succeed");
    let new_lock = lock_package_dir(&new_dir).expect("new package lock should succeed");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_eq!(old_review.summary.native_apis, 1);
    assert_eq!(new_review.summary.native_apis, 2);
    assert_ne!(
        old_lock.packages[0].review_hash,
        new_lock.packages[0].review_hash
    );
}

#[test]
fn package_lock_review_hash_tracks_await_live_across_changes() {
    let old_dir = common::unique_temp_dir("rsscript-package-lock-await-live-old");
    let new_dir = common::unique_temp_dir("rsscript-package-lock-await-live-new");
    let interface = r#"features: async, native

struct TimerError
struct Client

pub async native fn Timer.sleep(ms: Int) -> Result<Unit, TimerError>
    effects(native)

pub fn Log.done(client: read Client) -> Unit

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError>
"#;
    common::write_package_fixture(&old_dir, "0.1.0", "", interface);
    common::write_package_fixture(&new_dir, "0.1.0", "", interface);
    fs::create_dir_all(old_dir.join("src")).expect("old src dir should be created");
    fs::write(
        old_dir.join("src/main.rss"),
        r#"features: async

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError> {
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

pub async fn Api.run(client: read Client) -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    Log.done(client: read client)
    return Ok(Unit)
}
"#,
    )
    .expect("new source should be written");

    let old_lock = lock_package_dir(&old_dir).expect("old package lock should succeed");
    let new_lock = lock_package_dir(&new_dir).expect("new package lock should succeed");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_ne!(
        old_lock.packages[0].review_hash,
        new_lock.packages[0].review_hash
    );
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
fn package_lock_records_local_path_dependency_graph() {
    let root_dir = common::unique_temp_dir("rsscript-package-lock-graph-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-lock-graph-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );

    let lock = lock_package_dir(&root_dir).expect("package lock should include path deps");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_json(&lock))
        .expect("package lock JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(lock.packages.len(), 2);
    assert_eq!(json["package"][0]["name"], "rss-app");
    assert_eq!(json["package"][1]["name"], "rss-dep");
    assert_eq!(json["package"][1]["features"][0], "fast");
    assert!(
        json["package"][1]["interface_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );

    let lock_path = root_dir.join("rsspkg.lock");
    let reir_json: Value = serde_json::from_str(
        &rsscript::format_package_lock_reir_json_with_path(&lock, &lock_path),
    )
    .expect("package lock REIR JSON should parse");
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"].as_str() == Some("supply_chain")
                && fact["id"].as_str()
                    == Some("fact.lockfile.rss_dep_0_2_0.effective_interface_hash")
                && fact["acquisition_mode"].as_str() == Some("lockfile")
                && fact["evidence"][0]["kind"].as_str() == Some("lockfile_entry")
                && fact["evidence"][0]["file"] == lock_path.display().to_string()
                && fact["evidence"][0]["json_pointer"].as_str() == Some("/package/1/interface_hash")
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "package_risk_slice")
    }));
}

#[test]
fn package_lock_hashes_dependency_effective_interface_for_selected_features() {
    let root_base_dir = common::unique_temp_dir("rsscript-package-lock-feature-root-base");
    let root_fast_dir = common::unique_temp_dir("rsscript-package-lock-feature-root-fast");
    let dep_dir = common::unique_temp_dir("rsscript-package-lock-feature-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
fast = ["simd"]
simd = []

[interfaces.features.fast]
paths = ["interface/fast"]

[interfaces.features.simd]
paths = ["interface/simd"]
"#,
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    fs::create_dir_all(dep_dir.join("interface/fast"))
        .expect("feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/fast/lib.rssi"),
        r#"pub fn Dep.fast(text: read String) -> String
"#,
    )
    .expect("feature interface should be written");
    fs::create_dir_all(dep_dir.join("interface/simd"))
        .expect("transitive feature interface dir should be created");
    fs::write(
        dep_dir.join("interface/simd/lib.rssi"),
        r#"pub fn Dep.simd(text: read String) -> String
"#,
    )
    .expect("transitive feature interface should be written");
    common::write_named_package_fixture(
        &root_base_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    common::write_named_package_fixture(
        &root_fast_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["fast"] }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );

    let base_lock = lock_package_dir(&root_base_dir).expect("base lock should succeed");
    let fast_lock = lock_package_dir(&root_fast_dir).expect("fast lock should succeed");
    let _ = fs::remove_dir_all(&root_base_dir);
    let _ = fs::remove_dir_all(&root_fast_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    let base_dep = base_lock
        .packages
        .iter()
        .find(|package| package.name == "rss-dep")
        .expect("base dependency should be locked");
    let fast_dep = fast_lock
        .packages
        .iter()
        .find(|package| package.name == "rss-dep")
        .expect("fast dependency should be locked");
    assert_eq!(base_dep.features, Vec::<String>::new());
    assert_eq!(
        fast_dep.features,
        vec!["fast".to_string(), "simd".to_string()]
    );
    assert_ne!(base_dep.interface_hash, fast_dep.interface_hash);
}

#[test]
fn package_check_reports_stale_dependency_interface_lock() {
    let root_dir = common::unique_temp_dir("rsscript-package-check-dep-lock-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-check-dep-lock-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        "",
        r#"pub fn Dep.parse(text: read String) -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::write(
        dep_dir.join("interface/lib.rssi"),
        r#"pub fn Dep.parse(value: read String) -> String
"#,
    )
    .expect("dependency interface should be changed");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert_eq!(json["lock"]["matches"], false);
    assert!(
        json["lock"]["package_changes"]
            .as_array()
            .is_some_and(|changes| {
                changes.iter().any(|change| {
                    change["name"] == "rss-dep"
                        && change["changes"].as_array().is_some_and(|fields| {
                            fields.iter().any(|field| {
                                field["field"] == "interface_hash" && field["risk"] == "high"
                            })
                        })
                })
            })
    );
}

#[test]
fn package_check_reports_local_dependency_version_conflict() {
    let root_dir = common::unique_temp_dir("rsscript-package-check-conflict-root");
    let dep_a_dir = common::unique_temp_dir("rsscript-package-check-conflict-dep-a");
    let dep_b_dir = common::unique_temp_dir("rsscript-package-check-conflict-dep-b");
    let shared_v1_dir = common::unique_temp_dir("rsscript-package-check-conflict-shared-v1");
    let shared_v2_dir = common::unique_temp_dir("rsscript-package-check-conflict-shared-v2");
    common::write_named_package_fixture(
        &shared_v1_dir,
        "rss-shared",
        "0.1.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    common::write_named_package_fixture(
        &shared_v2_dir,
        "rss-shared",
        "0.2.0",
        "",
        r#"pub fn Shared.value() -> Int
"#,
    );
    common::write_named_package_fixture(
        &dep_a_dir,
        "rss-dep-a",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            common::toml_path(&shared_v1_dir)
        ),
        r#"pub fn DepA.run() -> Unit
"#,
    );
    common::write_named_package_fixture(
        &dep_b_dir,
        "rss-dep-b",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-shared = {{ path = "{}" }}
"#,
            common::toml_path(&shared_v2_dir)
        ),
        r#"pub fn DepB.run() -> Unit
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep-a = {{ path = "{}" }}
rss-dep-b = {{ path = "{}" }}
"#,
            common::toml_path(&dep_a_dir),
            common::toml_path(&dep_b_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_a_dir);
    let _ = fs::remove_dir_all(&dep_b_dir);
    let _ = fs::remove_dir_all(&shared_v1_dir);
    let _ = fs::remove_dir_all(&shared_v2_dir);

    assert!(!check.ok);
    assert_eq!(json["graph"]["ok"], false);
    assert_eq!(json["graph"]["risk"], "high");
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason
                .as_str()
                .is_some_and(|reason| reason.contains("rss-shared"))
        })
    }));
}

#[test]
fn package_review_update_reports_lockfile_contract_changes() {
    let old_dir = common::unique_temp_dir("rsscript-package-update-old");
    let new_dir = common::unique_temp_dir("rsscript-package-update-new");
    let lock_dir = common::unique_temp_dir("rsscript-package-update-locks");
    common::write_package_fixture(
        &old_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    common::write_package_fixture(
        &new_dir,
        "0.2.0",
        r#"[features]
fast = []
"#,
        r#"pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    );
    fs::create_dir_all(&lock_dir).expect("lock dir should be created");
    let old_lock_path = lock_dir.join("old.rsspkg.lock");
    let new_lock_path = lock_dir.join("new.rsspkg.lock");
    fs::write(
        &old_lock_path,
        format_package_lock_toml(&lock_package_dir(&old_dir).expect("old lock should be built")),
    )
    .expect("old lock should be written");
    fs::write(
        &new_lock_path,
        format_package_lock_toml(&lock_package_dir(&new_dir).expect("new lock should be built")),
    )
    .expect("new lock should be written");

    let diff =
        diff_package_locks(&old_lock_path, &new_lock_path).expect("lock diff should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_lock_diff_json(&diff))
        .expect("lock diff JSON should parse");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_lock_diff_reir_json(&diff))
            .expect("lock diff REIR JSON should parse");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);
    let _ = fs::remove_dir_all(&lock_dir);

    assert_eq!(json["risk"], "high");
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == ".rssi interface hash changed")
    }));
    assert!(json["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "package feature selection changed")
    }));
    assert!(json["package_changes"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["name"] == "rss-json"
                && change["changes"].as_array().is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| field["field"] == "interface_hash" && field["risk"] == "high")
                })
        })
    }));
    let interface_hash_after = json["package_changes"]
        .as_array()
        .and_then(|changes| {
            changes.iter().find_map(|change| {
                (change["name"] == "rss-json")
                    .then_some(change)
                    .and_then(|change| change["changes"].as_array())
                    .and_then(|fields| {
                        fields.iter().find_map(|field| {
                            (field["field"] == "interface_hash")
                                .then(|| field["after"].as_str())
                                .flatten()
                        })
                    })
            })
        })
        .expect("interface_hash after value should be reported");
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "supply_chain"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.contains(".field.interface_hash"))
                && fact["subject"]["id"] == "rss-json@0.2.0"
                && fact["value"] == true
                && fact["evidence"][0]["kind"] == "lockfile_entry"
                && fact["evidence"][0]["json_pointer"]
                    .as_str()
                    .is_some_and(|pointer| pointer.starts_with("/package_changes/0/changes/"))
                && fact["evidence"][0]["value"].as_str() == Some(interface_hash_after)
                && fact["evidence"][0]["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("risk=high"))
        }) && facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("fact.lock_update.") && id.ends_with(".risk"))
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "package_risk_slice")
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

#[test]
fn package_check_reports_stale_semantic_lock() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-stale");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    fs::write(
        temp_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&temp_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");
    fs::write(
        temp_dir.join("interface/lib.rssi"),
        r#"struct MathError

pub fn add(left: Int, right: Int) -> Result<Int, MathError>
"#,
    )
    .expect("interface should be changed");

    let check = check_package_dir(&temp_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let reir_json: Value = serde_json::from_str(&rsscript::format_package_check_reir_json(&check))
        .expect("package check REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!check.ok);
    assert_eq!(json["risk"], "high");
    assert_eq!(json["lock"]["present"], true);
    assert_eq!(json["lock"]["matches"], false);
    assert!(json["lock"]["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == ".rssi interface hash changed")
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"].as_str().is_some_and(|id| id.ends_with(".lock"))
                && fact["value"] == "unknown"
                && fact["acquisition_mode"] == "lockfile"
                && fact["evidence"][0]["kind"] == "lockfile_entry"
                && fact["evidence"][0]["file"] == json["lock"]["path"]
                && fact["evidence"][0]["json_pointer"] == "/lock"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "dependency_risk"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.contains(".lock_change."))
                && fact["evidence"][0]["kind"] == "lockfile_entry"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "supply_chain"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.contains(".field.interface_hash"))
                && fact["evidence"][0]["kind"] == "lockfile_entry"
                && fact["evidence"][0]["json_pointer"]
                    .as_str()
                    .is_some_and(|pointer| pointer.starts_with("/lock/package_changes/0/changes/"))
        })
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
fn package_check_reports_native_cargo_metadata() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-metadata");
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
    assert_eq!(json["native_rust"]["cargo_toml_present"], true);
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], true);
    assert_eq!(json["native_rust"]["cargo_package_name"], "rss_json_native");
    assert_eq!(json["native_rust"]["unsafe_detected"], false);
    assert_eq!(json["native_rust"]["build_env_detected"], false);
    assert_eq!(json["native_rust"]["build_download_detected"], false);
    assert_eq!(
        json["native_rust"]["linked_libraries"],
        serde_json::json!([])
    );
    assert!(
        json["native_rust"]["target_kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

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
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
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
        r#"[bindings]
"Native.ehco" = "rss_json_native::echo"
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("src/main.rss"),
        r#"features: native

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
        r#"[bindings]
"Native.echo" = "other_native::echo"
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
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
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
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
        r#"[bindings]
"Native.echo" = "rss_json_native::echo"
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
unsafe = "forbid"
"#,
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
    let reir_json: Value = serde_json::from_str(&rsscript::format_package_check_reir_json(&check))
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
    assert_eq!(check.risk, rsscript::PackageRisk::High);
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
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
fn package_check_reports_native_build_script_from_cargo_metadata() {
    let temp_dir = common::unique_temp_dir("rsscript-package-check-native-build-script");
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
        r#"features: native

native fn Native.parse(text: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(temp_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        temp_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_json_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(temp_dir.join("native/rust/build.rs"), "fn main() {}\n")
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
    assert_eq!(json["native_rust"]["cargo_metadata_ok"], true);
    assert!(
        json["native_rust"]["target_kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "custom-build"))
    );
    assert!(
        json["native_rust"]["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons
                .iter()
                .any(|reason| reason == "native Rust build script target present"))
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
    assert!(input.native_dependencies[0].path.ends_with("native/rust"));
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
        r#"features: native

native fn Native.echo(message: read String) -> String
    effects(native)
"#,
    );
    fs::create_dir_all(dep_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::create_dir_all(dep_dir.join("native")).expect("native dir should be created");
    fs::write(
        dep_dir.join("native/bindings.rssbind.toml"),
        r#"[bindings]
"Native.echo" = "rss_dep_native::echo"
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
        r#"features: native

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
fn package_lowering_input_records_checked_in_rayon_wrapper_dependency() {
    let root_dir = common::unique_temp_dir("rsscript-package-rust-rayon-root");
    let rayon_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("rss/rayon");
    common::write_named_package_fixture(
        &root_dir,
        "rss-rayon-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-rayon = {{ path = "{}" }}
"#,
            common::toml_path(&rayon_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"features: native

fn main() -> Unit {
    let values = List<Int>.new()
    List.push(list: mut values, value: read 1)
    List.push(list: mut values, value: read 2)
    List.push(list: mut values, value: read 3)
    let sum = Rayon.sum_squares(values: read values)
    Assert.equal_int(left: sum, right: 14)
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
    .expect("package source should lower with rayon native binding");
    let _ = fs::remove_dir_all(&root_dir);

    assert_eq!(input.native_dependencies.len(), 1);
    assert_eq!(input.native_dependencies[0].crate_name, "rss_rayon_native");
    assert!(
        input.native_dependencies[0]
            .bindings
            .get("Rayon.sum_squares")
            .is_some_and(|target| target == "rss_rayon_native::sum_squares")
    );
    assert!(
        package
            .cargo_toml
            .contains("\"rss_rayon_native\" = { path = ")
    );
    assert!(package.lib_rs.contains("rss_rayon_native::sum_squares"));
}

#[test]
fn package_vendor_can_emit_reir_supply_chain_facts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-vendor-reir");
    let root_dir = temp_dir.join("app");
    let dep_dir = temp_dir.join("dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-vendor-dep",
        "0.1.0",
        "",
        r#"pub fn Dep.value() -> Int
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-vendor-app",
        "0.1.0",
        r#"[dependencies]
rss-vendor-dep = { path = "../dep" }
rss-registry-dep = "^1"
"#,
        r#"pub fn App.run() -> Int
"#,
    );

    let vendor = vendor_package_dir(&root_dir, true).expect("vendor dry-run should succeed");
    let reir_json: Value =
        serde_json::from_str(&rsscript::format_package_vendor_reir_json(&vendor))
            .expect("vendor REIR JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(!vendor.ok);
    assert_eq!(vendor.entries.len(), 1);
    assert_eq!(vendor.unresolved.len(), 1);
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "supply_chain"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.contains(".entry.rss_vendor_dep_0_1_0.checksum"))
                && fact["subject"]["id"] == "rss-vendor-dep@0.1.0"
                && fact["evidence"][0]["kind"] == "package_metadata"
                && fact["evidence"][0]["file"] == vendor.entries[0].vendor_path
                && fact["evidence"][0]["json_pointer"] == "/entries/0"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "policy_result"
                && fact["id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with(".status"))
                && fact["evidence"][0]["file"] == vendor.vendor_dir
                && fact["evidence"][0]["json_pointer"] == "/ok"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "dependency_risk"
                && fact["subject"]["id"] == "rss-registry-dep@^1"
                && fact["value"] == "unknown"
                && fact["evidence"][0]["file"] == vendor.vendor_dir
                && fact["evidence"][0]["json_pointer"] == "/unresolved/0"
        })
    }));
}

#[test]
fn package_tree_expands_path_dependencies_and_marks_unresolved() {
    let root_dir = common::unique_temp_dir("rsscript-package-tree-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-tree-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.2.0",
        r#"[features]
streaming = []

[native.rust]
enabled = true
path = "native/rust"
crate = "rss_dep_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"
"#,
        r#"pub fn parse(text: read String) -> Result<fresh JsonValue, JsonError>
"#,
    );
    fs::create_dir_all(dep_dir.join("native/rust/src")).expect("native src dir should be created");
    fs::write(
        dep_dir.join("native/rust/Cargo.toml"),
        "[package]\nname = \"rss_dep_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("native Cargo.toml should be written");
    fs::write(
        dep_dir.join("native/rust/src/lib.rs"),
        "pub fn parse() {}\n",
    )
    .expect("native source should be written");
    common::write_package_fixture(
        &root_dir,
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}", features = ["streaming"] }}
rss-remote = "0.5"
rss-missing = {{ path = "../missing" }}
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn main() -> Unit
"#,
    );

    let tree = package_tree(&root_dir).expect("package tree should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_tree_json(&tree))
        .expect("package tree JSON should parse");
    let reir_json: Value = serde_json::from_str(&rsscript::format_package_tree_reir_json(&tree))
        .expect("package tree REIR JSON should parse");
    let human = rsscript::format_package_tree_human(&tree);
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert_eq!(json["root"]["name"], "rss-json");
    assert_eq!(json["summary"]["packages"], 4);
    assert_eq!(json["summary"]["path_dependencies"], 2);
    assert_eq!(json["summary"]["unresolved_dependencies"], 2);
    assert_eq!(json["summary"]["native_packages"], 1);
    assert!(json["root"]["dependencies"].as_array().is_some_and(|deps| {
        deps.iter().any(|dep| {
            dep["name"] == "rss-dep"
                && dep["version"] == "0.2.0"
                && dep["features"][0] == "streaming"
                && dep["native"] == true
        }) && deps
            .iter()
            .any(|dep| dep["name"] == "rss-remote" && dep["risk"] == "unknown")
            && deps
                .iter()
                .any(|dep| dep["name"] == "rss-missing" && dep["risk"] == "unknown")
    }));
    assert!(human.contains("|-- rss-dep 0.2.0 [elevated, native, features streaming]"));
    assert!(human.contains("`-- rss-remote req 0.5 [unknown]"));
    assert!(reir_json["producers"].as_array().is_some_and(|producers| {
        producers.iter().any(|producer| {
            producer["adapter"] == "rsscript-package-tree" && producer["source"] == "rsscript_tree"
        })
    }));
    assert!(reir_json["facts"].as_array().is_some_and(|facts| {
        facts.iter().any(|fact| {
            fact["kind"] == "dependency_risk"
                && fact["subject"]["id"] == "rss-dep@0.2.0"
                && fact["evidence"][0]["kind"] == "dependency_path"
                && fact["evidence"][0]["file"] == dep_dir.display().to_string()
                && fact["evidence"][0]["json_pointer"] == "/root/dependencies/0"
                && fact["evidence"][0]["source"] == "rsscript_tree"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "dependency_risk"
                && fact["value"] == "unknown"
                && fact["subject"]["id"] == "rss-remote@0.5"
                && fact["confidence"]["source"] == "rsscript_tree"
                && fact["evidence"][0]["file"].is_null()
                && fact["evidence"][0]["json_pointer"] == "/root/dependencies/2"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "dependency_risk"
                && fact["value"] == "unknown"
                && fact["subject"]["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("rss-missing@path+"))
                && fact["confidence"]["source"] == "rsscript_tree"
                && fact["evidence"][0]["file"].is_null()
                && fact["evidence"][0]["json_pointer"] == "/root/dependencies/1"
        }) && facts.iter().any(|fact| {
            fact["kind"] == "supply_chain"
                && fact["id"] == "fact.package_tree.root_1.effective_interface_hash"
                && fact["evidence"][0]["file"] == dep_dir.display().to_string()
                && fact["evidence"][0]["source"] == "rsscript_tree"
        })
    }));
    assert!(reir_json["edges"].as_array().is_some_and(|edges| {
        edges.iter().any(|edge| {
            edge["kind"] == "depends_on"
                && edge["from"]["id"] == "rss-json@0.1.0"
                && edge["to"]["id"] == "rss-dep@0.2.0"
                && edge["evidence"][0]["file"] == dep_dir.display().to_string()
                && edge["evidence"][0]["source"] == "rsscript_tree"
        })
    }));
    assert!(reir_json["slices"].as_array().is_some_and(|slices| {
        slices
            .iter()
            .any(|slice| slice["kind"] == "package_risk_slice")
    }));
}

#[test]
fn package_check_applies_dependency_graph_budgets() {
    let root_dir = common::unique_temp_dir("rsscript-package-graph-budget-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-graph-budget-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-dep",
        "0.1.0",
        "",
        r#"pub fn Dep.value() -> Int
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-dep = {{ path = "{}" }}

[dependency.budget]
max_direct_dependencies = 0
max_total_packages = 1
"#,
            common::toml_path(&dep_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert_eq!(json["graph"]["ok"], false);
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons
            .iter()
            .any(|reason| reason == "dependency graph exceeds direct dependencies budget: 1 > 0")
            && reasons
                .iter()
                .any(|reason| reason == "dependency graph exceeds total packages budget: 2 > 1")
    }));
}

#[test]
fn package_check_rejects_interface_only_dependency_without_provider() {
    let root_dir = common::unique_temp_dir("rsscript-package-provider-root");
    let interface_dir = common::unique_temp_dir("rsscript-package-interface-only-dep");
    common::write_named_package_fixture(
        &interface_dir,
        "platform-env",
        "0.1.0",
        "",
        r#"pub fn Env.home() -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
platform-env = {{ path = "{}" }}
"#,
            common::toml_path(&interface_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&interface_dir);

    assert!(!check.ok);
    assert_eq!(json["graph"]["ok"], false);
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason
                == "interface-only dependency `platform-env` requires an implementation provider for executable builds"
        })
    }));
}

#[test]
fn package_check_accepts_selected_provider_for_interface_only_dependency() {
    let root_dir = common::unique_temp_dir("rsscript-package-selected-provider-root");
    let interface_dir = common::unique_temp_dir("rsscript-package-selected-interface-dep");
    let provider_dir = common::unique_temp_dir("rsscript-package-selected-provider-dep");
    common::write_named_package_fixture(
        &interface_dir,
        "platform-env",
        "0.1.0",
        "",
        r#"pub fn Env.home() -> String
"#,
    );
    let interface_hash = lock_package_dir(&interface_dir)
        .expect("interface package should lock")
        .packages[0]
        .interface_hash
        .clone();
    common::write_named_package_fixture(
        &provider_dir,
        "posix-env",
        "0.1.0",
        &format!(
            r#"[implements."platform-env"]
version = "0.1"
interface_features = []
interface_effective_hash = "{}"
"#,
            interface_hash
        ),
        r#"pub fn PosixEnv.ready() -> Unit
"#,
    );
    fs::create_dir_all(provider_dir.join("src")).expect("provider source dir should be created");
    fs::write(
        provider_dir.join("src/lib.rss"),
        r#"pub fn PosixEnv.ready() -> Unit {
    return Unit
}
"#,
    )
    .expect("provider source should be written");
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
platform-env = {{ path = "{}" }}
posix-env = {{ path = "{}" }}

[providers]
platform-env = {{ package = "posix-env", version = "0.1.0" }}
"#,
            common::toml_path(&interface_dir),
            common::toml_path(&provider_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&interface_dir);
    let _ = fs::remove_dir_all(&provider_dir);

    assert!(check.ok);
    assert_eq!(json["graph"]["ok"], true);
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().all(|reason| {
            reason
                != "interface-only dependency `platform-env` requires an implementation provider for executable builds"
        })
    }));
}

#[test]
fn package_check_rejects_selected_provider_with_stale_interface_hash() {
    let root_dir = common::unique_temp_dir("rsscript-package-stale-provider-root");
    let interface_dir = common::unique_temp_dir("rsscript-package-stale-interface-dep");
    let provider_dir = common::unique_temp_dir("rsscript-package-stale-provider-dep");
    common::write_named_package_fixture(
        &interface_dir,
        "platform-env",
        "0.1.0",
        "",
        r#"pub fn Env.home() -> String
"#,
    );
    common::write_named_package_fixture(
        &provider_dir,
        "posix-env",
        "0.1.0",
        r#"[implements."platform-env"]
version = "0.1"
interface_features = []
interface_effective_hash = "sha256:stale"
"#,
        r#"pub fn PosixEnv.ready() -> Unit
"#,
    );
    fs::create_dir_all(provider_dir.join("src")).expect("provider source dir should be created");
    fs::write(
        provider_dir.join("src/lib.rss"),
        r#"pub fn PosixEnv.ready() -> Unit {
    return Unit
}
"#,
    )
    .expect("provider source should be written");
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
platform-env = {{ path = "{}" }}
posix-env = {{ path = "{}" }}

[providers]
platform-env = {{ package = "posix-env", version = "0.1.0" }}
"#,
            common::toml_path(&interface_dir),
            common::toml_path(&provider_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&interface_dir);
    let _ = fs::remove_dir_all(&provider_dir);

    assert!(!check.ok);
    assert_eq!(json["graph"]["ok"], false);
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason
                == "provider `posix-env` interface hash for `platform-env` is stale or mismatched"
        })
    }));
}

#[test]
fn package_check_allows_platform_provided_interface_only_dependency() {
    let root_dir = common::unique_temp_dir("rsscript-package-platform-provider-root");
    let interface_dir = common::unique_temp_dir("rsscript-package-platform-interface-dep");
    common::write_named_package_fixture(
        &interface_dir,
        "platform-env",
        "0.1.0",
        "",
        r#"pub fn Env.home() -> String
"#,
    );
    common::write_named_package_fixture(
        &root_dir,
        "rss-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
platform-env = {{ path = "{}", platform_provided = true }}
"#,
            common::toml_path(&interface_dir)
        ),
        r#"pub fn App.run() -> Unit
"#,
    );
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&interface_dir);

    assert!(check.ok);
    assert_eq!(json["graph"]["ok"], true);
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().all(|reason| {
            reason
                != "interface-only dependency `platform-env` requires an implementation provider for executable builds"
        })
    }));
}
