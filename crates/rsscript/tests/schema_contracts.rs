use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn schema(name: &str) -> serde_json::Value {
    let path = workspace_root().join("schemas").join(name);
    serde_json::from_str(
        &fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn assert_valid(schema: &serde_json::Value, instance: &serde_json::Value) {
    let validator = jsonschema::validator_for(schema).expect("schema must be valid");
    let errors = validator
        .iter_errors(instance)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "schema errors: {errors:#?}");
}

#[test]
fn package_build_outputs_match_versioned_analysis_and_bytecode_schemas() {
    let package =
        workspace_root().join("crates/rsscript/tests/fixtures/pass/selfhost-package-source-set");
    let snapshot = rsscript::load_workspace_snapshot(&package).expect("snapshot");
    let mut executable =
        rsscript::reg_vm_compile_package_input(snapshot.lowering_input()).expect("compile");
    executable
        .bind_snapshot_digest(snapshot.digest())
        .expect("bind snapshot");
    let mut analysis = snapshot.analysis().clone();
    analysis.module_digest = Some(
        executable
            .bytecode_artifact()
            .header
            .executable_hash
            .clone(),
    );

    assert_valid(
        &schema("rsscript-package-analysis-v1.json"),
        &serde_json::to_value(analysis).expect("analysis JSON"),
    );
    assert_valid(
        &schema("rsscript.bytecode.v1.schema.json"),
        &serde_json::to_value(executable.bytecode_artifact()).expect("bytecode logical JSON"),
    );
}

#[test]
fn checked_in_binding_descriptor_matches_its_schema() {
    let path = workspace_root().join("packages/native-abi-fixture/native/bindings.rssbind.toml");
    let descriptor: toml::Value = toml::from_str(
        &fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .expect("binding TOML");
    assert_valid(
        &schema("rsscript-bindings-v1.json"),
        &serde_json::to_value(descriptor).expect("binding JSON projection"),
    );
}

#[test]
fn public_schemas_reject_unknown_top_level_fields() {
    for name in [
        "rsscript-package-analysis-v1.json",
        "rsscript.bytecode.v1.schema.json",
        "rsscript-bindings-v1.json",
        "rsscript.execution_report.v1.schema.json",
        "rsscript.core_metrics.v1.schema.json",
    ] {
        let schema = schema(name);
        let validator = jsonschema::validator_for(&schema).expect("schema must be valid");
        assert!(!validator.is_valid(&serde_json::json!({"unexpected": true})));
    }
}

#[test]
fn binding_schema_rejects_unknown_function_fields() {
    let schema = schema("rsscript-bindings-v1.json");
    let validator = jsonschema::validator_for(&schema).expect("schema must be valid");
    assert!(!validator.is_valid(&serde_json::json!({
        "schema": "rsscript.bindings.v1",
        "function": [{
            "symbol": "host.log.emit",
            "provider": "rsscript.log",
            "entry": "emit",
            "unexpected": true
        }]
    })));
}
