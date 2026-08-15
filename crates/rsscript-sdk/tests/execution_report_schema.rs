#![cfg(feature = "execution")]

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn load_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(
        &fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn execution_report_v1_schema_retains_historical_golden_reports() {
    let root = workspace_root();
    let schema = load_json(&root.join("schemas/rsscript.execution_report.v1.schema.json"));
    let validator = jsonschema::validator_for(&schema).expect("execution report schema");

    for fixture in [
        "completed.json",
        "cancelled.json",
        "step-budget.json",
        "provider-error.json",
    ] {
        let report = load_json(&root.join("schemas/fixtures/execution-report").join(fixture));
        let errors = validator
            .iter_errors(&report)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "{fixture} schema errors: {errors:#?}");
        if let Some(failure) = report["failure"].as_object() {
            assert_eq!(report["termination_reason"], failure["reason"]);
        }
    }
}

#[test]
fn execution_report_v2_schema_accepts_live_output_without_native_value() {
    let root = workspace_root();
    let schema = load_json(&root.join("schemas/rsscript.execution_report.v2.schema.json"));
    let validator = jsonschema::validator_for(&schema).expect("execution report schema");
    let compiler = rsscript_sdk::Compiler;
    let package = compiler
        .compile("main.rss", "fn main() -> Unit { return Unit }")
        .expect("compile live report fixture");
    let package = rsscript_sdk::ArtifactVerifier
        .verify(package)
        .expect("verify live report fixture")
        .admit_trusted_input();
    let report = rsscript_sdk::Runtime::default()
        .link(&package)
        .expect("link live report fixture")
        .execute(rsscript_sdk::ExecutionRequest::default());
    let report = serde_json::to_value(report).expect("serialize live report");
    let errors = validator
        .iter_errors(&report)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "live report schema errors: {errors:#?}");
    assert_eq!(report["schema"], "rsscript.execution_report.v2");
    assert_eq!(report["outcome"]["kind"], "completed");
    assert_eq!(report["outcome"]["wire_value"]["kind"], "unit");
    assert!(report.get("native_value").is_none());
}

#[test]
fn execution_report_schema_is_fail_closed() {
    let root = workspace_root();
    let schema = load_json(&root.join("schemas/rsscript.execution_report.v2.schema.json"));
    let validator = jsonschema::validator_for(&schema).expect("execution report schema");
    let mut report = serde_json::json!({
        "schema": "rsscript.execution_report.v2",
        "artifact_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "outcome": {
            "kind": "completed",
            "wire_value": { "kind": "int", "value": 42 },
            "display_value": "42"
        },
        "usage": {},
        "telemetry": {},
        "stdout": "",
        "stderr": "",
        "provider_call_traces": [],
        "diagnostics": []
    });

    report["unexpected"] = serde_json::json!(true);
    assert!(!validator.is_valid(&report));

    let mut mismatched = report;
    mismatched
        .as_object_mut()
        .expect("object")
        .remove("unexpected");
    mismatched["outcome"]["wire_value"] = serde_json::json!({
        "kind": "record",
        "type_id": 1,
        "fields": [],
        "unexpected": true
    });
    assert!(!validator.is_valid(&mismatched));
}

#[test]
fn semantic_diff_schema_accepts_live_policy_neutral_output() {
    let root = workspace_root();
    let schema = load_json(&root.join("schemas/rsscript.semantic_diff.v2.schema.json"));
    let validator = jsonschema::validator_for(&schema).expect("semantic diff schema");
    let compiler = rsscript_sdk::Compiler;
    let old = compiler
        .compile("old.rss", "fn main() -> Int { return 1 }")
        .expect("old build");
    let new = compiler
        .compile("new.rss", "fn main() -> Int { return 2 }")
        .expect("new build");
    let diff = rsscript_sdk::SemanticDiffV1::between(old.bundle(), new.bundle());
    let value = serde_json::to_value(diff).expect("serialize semantic diff");
    let errors = validator
        .iter_errors(&value)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "semantic diff schema errors: {errors:#?}"
    );
    let text = serde_json::to_string(&value).unwrap();
    assert!(!text.contains("risk"));
    assert!(!text.contains("allow"));
    assert!(!text.contains("deny"));
}

#[test]
fn semantic_diff_json_and_markdown_goldens_remain_policy_neutral() {
    let root = workspace_root();
    let json_path = root.join("schemas/fixtures/semantic-diff/structural-evidence.v2.json");
    let markdown_path = root.join("schemas/fixtures/semantic-diff/structural-evidence.v2.md");
    let json = fs::read_to_string(&json_path).expect("read semantic diff JSON golden");
    let diff: rsscript_sdk::SemanticDiffV1 =
        serde_json::from_str(&json).expect("deserialize semantic diff JSON golden");
    let normalized = serde_json::to_value(&diff).expect("serialize semantic diff golden");
    let expected = load_json(&json_path);
    assert_eq!(
        normalized, expected,
        "semantic diff JSON golden must round trip"
    );
    let schema = load_json(&root.join("schemas/rsscript.semantic_diff.v2.schema.json"));
    let validator = jsonschema::validator_for(&schema).expect("semantic diff schema");
    assert!(
        validator.is_valid(&normalized),
        "semantic diff golden must validate"
    );
    let mut with_policy = normalized.clone();
    with_policy["risk"] = serde_json::json!("high");
    assert!(
        !validator.is_valid(&with_policy),
        "neutral semantic diff schema must reject a policy field"
    );
    assert_eq!(
        diff.to_markdown(),
        fs::read_to_string(markdown_path).expect("read Markdown golden")
    );

    let text = serde_json::to_string(&normalized).expect("serialize semantic diff output");
    for forbidden in ["risk", "allow", "deny", "policy", "verdict"] {
        assert!(
            !text.contains(forbidden),
            "semantic diff must only report facts, found forbidden term `{forbidden}`"
        );
    }
}
