//! Spec §2.5 — package review surface and risk
#![allow(unused_imports, dead_code)]
use super::*;

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
    assert_eq!(json["risk"], "unknown");
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

    assert_eq!(bundle["schema"], "reir.bundle.v0.2");
    assert_eq!(bundle["ontology"], "reir.capability_ontology.v0.2");
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
fn package_review_rejects_source_path_escape() {
    let temp_dir = common::unique_temp_dir("rsscript-package-source-path-escape");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-path-escape"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[sources]
paths = ["../outside"]
"#,
    )
    .expect("manifest should be written");
    fs::write(
        temp_dir.join("interface/lib.rssi"),
        r#"pub fn App.run() -> Unit
"#,
    )
    .expect("interface should be written");

    let error = review_package_dir(&temp_dir).expect_err("escaped source root should fail");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(error.contains("source root `../outside` must not escape the package root"));
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
fn package_review_capability_propagates_through_hir_resolved_receiver_call() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-receiver-capability");
    fs::create_dir_all(temp_dir.join("interface")).expect("interface dir should be created");
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("rsspkg.toml"),
        r#"[package]
name = "rss-receiver-capability"
version = "0.1.0"
edition = "2026"

[interfaces]
paths = ["interface"]

[sources]
paths = ["src"]

[[review.capability_bindings]]
symbol = "S3Client.put_object"
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

opaque class S3Client

native fn S3Client.put_object(
    self: read S3Client,
    body: read String,
) -> Result<Unit, String>
    effects(native)
"#,
    )
    .expect("interface should be written");
    fs::write(
        temp_dir.join("src/upload.rss"),
        r#"pub fn upload_report(client: read S3Client, report: read String) -> Result<Unit, String> {
    return read client.put_object(body: read report)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let review_json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        review_json["capabilities"]
            .as_array()
            .is_some_and(|capabilities| {
                capabilities.iter().any(|capability| {
                    capability["function"] == "upload_report"
                        && capability["binding_symbol"] == "S3Client.put_object"
                        && capability["action"] == "s3:PutObject"
                        && capability["call_chain"].as_array().is_some_and(|chain| {
                            chain
                                == &vec![
                                    Value::from("upload_report"),
                                    Value::from("S3Client.put_object"),
                                ]
                        })
                })
            }),
        "{review_json:#}"
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
fn package_review_reir_maps_file_json_toml_and_yaml_facades_to_filesystem_capabilities() {
    let temp_dir =
        common::unique_temp_dir("rsscript-package-review-file-json-toml-yaml-facades-reir");
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

pub fn Yaml.parse_file(path: read Path) -> Result<fresh JsonValue, JsonError>
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
            "Yaml.parse_file",
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
fn reir_spec_keeps_os_close_as_descriptor_cleanup_not_external_capability() {
    let root = common::workspace_root();
    let spec = fs::read_to_string(root.join("docs/spec/Review_Evidence_IR_Spec_v0.2.md"))
        .expect("REIR spec should be readable");

    assert!(spec.contains("`OS.close`"));
    assert!(spec.contains("trusted native/resource"));
    assert!(spec.contains("do not imply `filesystem.read`, `filesystem.write`, or"));
}

#[test]
fn package_review_markdown_lists_capabilities_by_risk() {
    let review =
        review_package_dir(&common::workspace_root().join("examples/capability-review-demo/after"))
            .expect("demo review should succeed");
    let markdown = rsscript::format_package_review_markdown(&review);
    assert!(markdown.contains("## RSScript review:"));
    assert!(markdown.contains("### Capabilities (by risk)"));
    assert!(markdown.contains("network.client"));
    assert!(markdown.contains("database.read"));
    // high-risk capability is listed before medium.
    let high = markdown.find("network.client").unwrap();
    let medium = markdown.find("database.read").unwrap();
    assert!(high < medium, "high-risk capability should sort first");
}
