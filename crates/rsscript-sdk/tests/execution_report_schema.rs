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
fn execution_report_schema_accepts_golden_reports_and_live_output() {
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

    let compiler = rsscript_sdk::Compiler;
    let package = compiler
        .compile("main.rss", "fn main() -> Unit { return Unit }")
        .expect("compile live report fixture");
    let package = rsscript_sdk::ArtifactVerifier
        .verify(package)
        .expect("verify live report fixture");
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
}

#[test]
fn execution_report_schema_is_fail_closed() {
    let root = workspace_root();
    let schema = load_json(&root.join("schemas/rsscript.execution_report.v1.schema.json"));
    let validator = jsonschema::validator_for(&schema).expect("execution report schema");
    let mut report = load_json(&root.join("schemas/fixtures/execution-report/completed.json"));

    report["unexpected"] = serde_json::json!(true);
    assert!(!validator.is_valid(&report));

    let mut mismatched = load_json(&root.join("schemas/fixtures/execution-report/cancelled.json"));
    mismatched["termination_reason"] = serde_json::json!("unknown_reason");
    assert!(!validator.is_valid(&mismatched));
}

#[test]
fn semantic_diff_schema_accepts_live_policy_neutral_output() {
    let root = workspace_root();
    let schema = load_json(&root.join("schemas/rsscript.semantic_diff.v1.schema.json"));
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
