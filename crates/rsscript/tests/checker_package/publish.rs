//! Spec §2.5 — pre-publish checks
#![allow(unused_imports, dead_code)]
use super::*;

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
        "{\"schema\":\"reir.bundle.v0.2\",\"producer\":\"check\"}",
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
        "{\"schema\":\"reir.bundle.v0.2\",\"producer\":\"metadata\"}",
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

#[cfg(unix)]
#[test]
fn package_publish_archive_rejects_symlink_entries() {
    use std::os::unix::fs::symlink;

    let temp_dir = common::unique_temp_dir("rsscript-package-publish-rejects-symlink");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-publish-symlink",
        "0.1.0",
        "",
        r#"pub fn add(left: Int, right: Int) -> Int
"#,
    );
    let outside_file = temp_dir.with_extension("outside.txt");
    fs::write(&outside_file, "secret").expect("outside file should be written");
    symlink(&outside_file, temp_dir.join("leak.txt")).expect("symlink should be created");

    let error = publish_package_dry_run(&temp_dir).expect_err("publish should reject symlinks");
    let _ = fs::remove_file(&outside_file);
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(error.contains("package archive rejects symlinks"));
    assert!(error.contains("leak.txt"));
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
