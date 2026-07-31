//! Spec §2.5/§6 — .rssi contracts and source sets
#![allow(unused_imports, dead_code)]
use super::*;

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
fn package_review_exports_deprecation_reason() {
    let temp_dir = common::unique_temp_dir("rsscript-package-deprecated-export");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-deprecated-export",
        "0.1.0",
        "",
        r#"#deprecated("use Render.render_v2")
pub fn Render.render(body: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"#deprecated("use Render.render_v2")
pub fn Render.render(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(review.diagnostics.is_empty(), "{:?}", review.diagnostics);
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports.iter().any(|export| {
            export["name"] == "Render.render"
                && export["kind"] == "function"
                && export["reasons"].as_array().is_some_and(|reasons| {
                    reasons
                        .iter()
                        .any(|reason| reason == "deprecated: use Render.render_v2")
                })
        })
    }));
}

#[test]
fn package_tests_can_see_package_internals_without_exporting_them() {
    let temp_dir = common::unique_temp_dir("rsscript-package-test-scope");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-test-scope",
        "0.1.0",
        r#"[tests]
paths = ["tests"]
"#,
        r#"pub fn Public.answer() -> Int
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("tests")).expect("tests dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"fn private_answer() -> Int {
    return 41
}

pub fn Public.answer() -> Int {
    return private_answer() + 1
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("tests/private.rss"),
        r#"fn test_private_answer() -> Unit {
    Assert.equal_int(left: private_answer(), right: 41)
}
"#,
    )
    .expect("test source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript::format_package_review_json(&review))
        .expect("package review JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(review.diagnostics.is_empty(), "{:?}", review.diagnostics);
    assert!(json["files"].as_array().is_some_and(|files| {
        files.iter().any(|file| {
            file["kind"] == "test"
                && file["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("tests/private.rss"))
        })
    }));
    assert!(json["exports"].as_array().is_some_and(|exports| {
        exports
            .iter()
            .any(|export| export["name"] == "Public.answer" && export["kind"] == "function")
            && !exports
                .iter()
                .any(|export| export["name"] == "private_answer")
            && !exports
                .iter()
                .any(|export| export["name"] == "test_private_answer")
    }));
}

#[test]
fn package_review_reports_deprecation_reason_contract_mismatch() {
    let temp_dir = common::unique_temp_dir("rsscript-package-deprecated-mismatch");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-deprecated-mismatch",
        "0.1.0",
        "",
        r#"#deprecated("use Render.render_v2")
pub fn Render.render(body: read String) -> String
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"#deprecated("use Render.render_fast")
pub fn Render.render(body: read String) -> String {
    return body
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let causes = review
        .diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.causes.iter())
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(review.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "RS1301" && diagnostic.label == "interface/source signature mismatch"
    }));
    assert!(
        causes.iter().any(|cause| {
            cause.contains(
                "interface: pub fn Render.render(body: read String) -> String #deprecated(\"use Render.render_v2\")",
            )
        }),
        "{causes:?}"
    );
    assert!(
        causes.iter().any(|cause| {
            cause.contains(
                "source: pub fn Render.render(body: read String) -> String #deprecated(\"use Render.render_fast\")",
            )
        }),
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
fn package_review_includes_lint_warnings_for_public_contracts() {
    let temp_dir = common::unique_temp_dir("rsscript-package-review-lint");
    common::write_package_fixture(
        &temp_dir,
        "0.1.0",
        "",
        r#"features: native

struct Error
struct Picture

pub fn Api.overloaded<A, B, C, D>(
    first: read Result<Option<List<Map<String, Picture>>>, Error>,
    second: read String,
    third: read String,
    fourth: read String,
    fifth: read String,
    sixth: read String,
    seventh: read String,
) -> Result<Option<List<Map<String, Picture>>>, Error>
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

    fn flush(self: mut Self) -> Unit = _
}

struct BufferWriter

pub fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message))

pub fn BufferWriter.flush(self: mut BufferWriter) -> Unit

impl Writer for BufferWriter {
    write = BufferWriter.write
    flush = BufferWriter.flush
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
                        && reasons.iter().any(|reason| reason == "method `flush`")
                        && reasons.iter().any(|reason| {
                            reason
                                == "method contract `fn Writer.flush(self: mut Self) -> Unit = _`"
                        })
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
