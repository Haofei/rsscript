//! Spec §2.5 — dependency contracts and versions
#![allow(unused_imports, dead_code)]
use super::*;

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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
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
fn package_check_reports_unknown_dependency_table_key() {
    let root_dir = common::unique_temp_dir("rsscript-package-dep-unknown-key-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-dep-unknown-key-dep");
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
rss-dep = {{ path = "{}", typo = true }}
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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["summary"] == "dependency `rss-dep` has unknown key `typo`."
                && diagnostic["label"] == "unknown dependency key"
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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
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
fn package_tests_see_dev_dependency_interfaces_not_internals() {
    let temp_dir = common::unique_temp_dir("rsscript-package-test-dev-scope");
    let dep_dir = temp_dir.join("deps/helper");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-helper",
        "0.1.0",
        "",
        r#"pub fn Helper.public() -> Int
"#,
    );
    fs::create_dir_all(dep_dir.join("src")).expect("dependency source dir should be created");
    fs::write(
        dep_dir.join("src/lib.rss"),
        r#"fn Helper.secret() -> Int {
    return 7
}

pub fn Helper.public() -> Int {
    return Helper.secret()
}
"#,
    )
    .expect("dependency source should be written");

    common::write_named_package_fixture(
        &temp_dir,
        "rss-test-dev-scope",
        "0.1.0",
        r#"[tests]
paths = ["tests"]

[dev-dependencies.helper]
path = "deps/helper"
"#,
        r#"pub fn Public.answer() -> Int
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::create_dir_all(temp_dir.join("tests")).expect("tests dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub fn Public.answer() -> Int {
    return 1
}
"#,
    )
    .expect("source should be written");
    fs::write(
        temp_dir.join("tests/dev_dep.rss"),
        r#"fn test_dev_dependency_scope() -> Unit {
    Assert.equal_int(left: Helper.public(), right: 7)
    Assert.equal_int(left: Helper.secret(), right: 7)
}
"#,
    )
    .expect("test source should be written");

    let review = review_package_dir(&temp_dir).expect("package review should succeed");
    let diagnostics = review
        .diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{}:{}:{}",
                diagnostic.code, diagnostic.label, diagnostic.summary
            )
        })
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("Helper.secret")),
        "{diagnostics:?}"
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("Helper.public")),
        "{diagnostics:?}"
    );
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
fn package_review_map_resolves_path_dependency_interfaces() {
    let dep_dir = common::unique_temp_dir("rsscript-package-review-map-dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-review-dep",
        "0.1.0",
        "",
        r#"
fn Dep.echo(message: read String) -> String
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
        r#"
pub fn Api.run(message: read String) -> String {
    return Dep.echo(message: read message)
}
"#,
    )
    .expect("source should be written");

    let review = review_package_dir(&root_dir).expect("package review should succeed");
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_review_json(&review))
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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
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
fn package_lowering_input_includes_path_dependency_sources() {
    let root_dir = common::unique_temp_dir("rsscript-package-source-dep-root");
    let dep_dir = common::unique_temp_dir("rsscript-package-source-dep-lib");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-helper-dep",
        "0.1.0",
        "",
        r#"pub fn Helper.answer() -> Int
"#,
    );
    fs::create_dir_all(dep_dir.join("src")).expect("dep source dir should be created");
    fs::write(
        dep_dir.join("src/lib.rss"),
        r#"pub fn Helper.answer() -> Int {
    return 42
}
"#,
    )
    .expect("dep source should be written");

    common::write_named_package_fixture(
        &root_dir,
        "rss-helper-root",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-helper-dep = {{ path = "{}" }}
"#,
            common::toml_path(&dep_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("root source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"fn main() -> Unit {
    let answer = Helper.answer()
    Assert.equal_int(left: answer, right: 42)
    return Unit
}
"#,
    )
    .expect("root source should be written");

    let input = package_lowering_input(&root_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with dependency source");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&dep_dir);

    assert!(input.sources.iter().any(|(path, source)| {
        path.contains("rsscript-package-source-dep-lib") && source.contains("Helper.answer")
    }));
    assert!(package.lib_rs.contains("fn Helper_answer"));
}

#[test]
fn package_requires_explicit_async_dependency_for_timer_api() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-missing-dependency");
    common::write_named_package_fixture(&root_dir, "rss-async-app", "0.1.0", "", "");
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"
async fn main() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should complete");
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["summary"]
                .as_str()
                .is_some_and(|summary| summary.contains("unknown type `TimerError`"))
                || diagnostic["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("unresolved function `Timer.sleep`"))
        })
    }));
}

#[test]
fn package_requires_explicit_async_dependency_for_async_file_io() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-file-missing-dependency");
    common::write_named_package_fixture(&root_dir, "rss-async-file-app", "0.1.0", "", "");
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"
async fn load(path: read Path) -> Result<String, FileError> {
    let text = await File.read_all_string_async(path: read path)?
    return Ok(text)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should complete");
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["summary"].as_str().is_some_and(|summary| {
                summary.contains("call to `File.read_all_string_async` does not resolve")
            })
        })
    }));
}

#[test]
fn package_requires_explicit_async_dependency_for_async_process_io() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-process-missing-dependency");
    common::write_named_package_fixture(&root_dir, "rss-async-process-app", "0.1.0", "", "");
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"
async fn run_command() -> Result<String, String> {
    let stdout = await Process.run_stdout_async(command: read "printf", args: read List<String>.new())?
    return Ok(stdout)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should complete");
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["summary"].as_str().is_some_and(|summary| {
                summary.contains("call to `Process.run_stdout_async` does not resolve")
            })
        })
    }));
}

#[test]
fn package_requires_explicit_async_dependency_for_async_tcp_io() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-tcp-missing-dependency");
    common::write_named_package_fixture(&root_dir, "rss-async-tcp-app", "0.1.0", "", "");
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"
async fn connect() -> Result<TcpStream, TcpError> {
    let stream = await Tcp.connect(host: read "127.0.0.1", port: 8080)?
    return Ok(stream)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should complete");
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["summary"].as_str().is_some_and(|summary| {
                summary.contains("unknown type `TcpStream`")
                    || summary.contains("call to `Tcp.connect` does not resolve")
            })
        })
    }));
}

#[test]
fn package_requires_explicit_async_dependency_for_async_websocket_io() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-websocket-missing-dependency");
    common::write_named_package_fixture(&root_dir, "rss-async-websocket-app", "0.1.0", "", "");
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"
async fn connect(url: read Url) -> Result<WebSocket, WebSocketError> {
    let socket = await WebSocket.connect(url: read url)?
    return Ok(socket)
}
"#,
    )
    .expect("source should be written");
    fs::write(
        root_dir.join("rsspkg.lock"),
        format_package_lock_toml(
            &lock_package_dir(&root_dir).expect("initial lock should be generated"),
        ),
    )
    .expect("lock should be written");

    let check = check_package_dir(&root_dir).expect("package check should complete");
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(!check.ok);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["summary"].as_str().is_some_and(|summary| {
                summary.contains("unknown type `WebSocket`")
                    || summary.contains("call to `WebSocket.connect` does not resolve")
            })
        })
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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
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
    let json: Value = serde_json::from_str(&rsscript_sdk::format_package_check_json(&check))
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
