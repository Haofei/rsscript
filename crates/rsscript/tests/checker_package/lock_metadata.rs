//! Spec §2.5 — semantic lock, metadata, vendor, tree, providers
#![allow(unused_imports, dead_code)]
use super::*;

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
    assert_eq!(reir["schema"], "reir.bundle.v0.2");
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
fn package_manager_spec_uses_implemented_provider_resolution_manifest_shape() {
    let spec = fs::read_to_string(common::package_manager_spec_path())
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
fn package_lock_review_hash_tracks_capability_provider_swap() {
    // Same code, same capability category — only the provider changes. The lock
    // must still notice (provider pinning), otherwise a supply-chain provider
    // swap would be invisible to review.
    let old_dir = common::unique_temp_dir("rsscript-package-lock-provider-old");
    let new_dir = common::unique_temp_dir("rsscript-package-lock-provider-new");
    let manifest = |provider: &str| {
        format!(
            r#"[native.rust]
enabled = true
path = "native/rust"
crate = "rss_native"
build_scripts = "forbid"
proc_macros = "forbid"
unsafe = "forbid"

[[review.capability_bindings]]
symbol = "Native.write"
category = "database.write"
provider = "{provider}"
"#
        )
    };
    let source = r#"features: native

native fn Native.write(message: read String) -> String
    effects(native)
"#;
    let bindings = r#"[bindings]
"Native.write" = "rss_native::write"
"#;
    common::write_package_fixture(&old_dir, "0.1.0", &manifest("trusted-db"), source);
    common::write_package_fixture(&new_dir, "0.1.0", &manifest("rogue-db"), source);
    for dir in [&old_dir, &new_dir] {
        fs::create_dir_all(dir.join("native")).expect("native dir should be created");
        fs::write(dir.join("native/bindings.rssbind.toml"), bindings)
            .expect("native bindings should be written");
    }

    let old_lock = lock_package_dir(&old_dir).expect("old package lock should succeed");
    let new_lock = lock_package_dir(&new_dir).expect("new package lock should succeed");
    let _ = fs::remove_dir_all(&old_dir);
    let _ = fs::remove_dir_all(&new_dir);

    assert_ne!(
        old_lock.packages[0].review_hash, new_lock.packages[0].review_hash,
        "a capability provider swap must change the review hash"
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
fn package_check_rejects_async_interface_dependency_without_provider() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-no-provider");
    let repo = common::workspace_root();
    let async_dir = repo.join("packages/async");
    common::write_named_package_fixture(
        &root_dir,
        "rss-async-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-async = {{ path = "{}" }}
"#,
            common::toml_path(&async_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"features: async

async fn main() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}

async fn load(path: read Path) -> Result<String, FileError> {
    await File.write_string_async(path: read path, text: read "hello")?
    let text = await File.read_all_string_async(path: read path)?
    return Ok(text)
}

async fn fetch(url: read Url) -> Result<Int, HttpError> {
    let response = await Http.get_async(url: read url)?
    return Ok(HttpResponse.status(response: read response))
}

fn stream_file(path: read Path) -> Result<Unit, ChannelError> {
    let chunks: Stream<Bytes> = File.bytes_stream(path: read path, chunk_size: 4096)?
    let rows: Stream<Row> = Csv.rows(path: read path, buffer_size: 8192)?
    return Ok(Unit)
}

async fn run_command() -> Result<String, String> {
    let stdout = await Process.run_stdout_async(command: read "printf", args: read List<String>.new())?
    return Ok(stdout)
}

async fn connect_tcp() -> Result<TcpStream, TcpError> {
    let stream = await Tcp.connect(host: read "127.0.0.1", port: 8080)?
    return Ok(stream)
}

async fn connect_websocket(url: read Url) -> Result<WebSocket, WebSocketError> {
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
    let json: Value = serde_json::from_str(&rsscript::format_package_check_json(&check))
        .expect("package check JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(!check.ok);
    assert!(json["graph"]["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|reason| {
            reason
                == "interface-only dependency `rss-async` requires an implementation provider for executable builds"
        })
    }));
}

#[test]
fn package_check_accepts_async_runtime_provider_and_lowers_timer_api() {
    let root_dir = common::unique_temp_dir("rsscript-package-async-provider");
    let repo = common::workspace_root();
    let async_dir = repo.join("packages/async");
    let async_runtime_dir = repo.join("packages/async-runtime");
    common::write_named_package_fixture(
        &root_dir,
        "rss-async-app",
        "0.1.0",
        &format!(
            r#"[dependencies]
rss-async = {{ path = "{}" }}
rss-async-runtime = {{ path = "{}" }}

[providers]
async = "rss-async-runtime"
"#,
            common::toml_path(&async_dir),
            common::toml_path(&async_runtime_dir)
        ),
        "",
    );
    fs::create_dir_all(root_dir.join("src")).expect("source dir should be created");
    fs::write(
        root_dir.join("src/main.rss"),
        r#"features: async

async fn main() -> Result<Unit, TimerError> {
    await Timer.sleep(ms: 1)?
    return Ok(Unit)
}

async fn load(path: read Path) -> Result<String, FileError> {
    await File.write_string_async(path: read path, text: read "hello")?
    let text = await File.read_all_string_async(path: read path)?
    return Ok(text)
}

async fn fetch(url: read Url) -> Result<Int, HttpError> {
    let response = await Http.get_async(url: read url)?
    return Ok(HttpResponse.status(response: read response))
}

fn stream_file(path: read Path) -> Result<Unit, ChannelError> {
    let chunks: Stream<Bytes> = File.bytes_stream(path: read path, chunk_size: 4096)?
    let rows: Stream<Row> = Csv.rows(path: read path, buffer_size: 8192)?
    return Ok(Unit)
}

async fn run_command() -> Result<String, String> {
    let stdout = await Process.run_stdout_async(command: read "printf", args: read List<String>.new())?
    return Ok(stdout)
}

async fn connect_tcp() -> Result<TcpStream, TcpError> {
    let stream = await Tcp.connect(host: read "127.0.0.1", port: 8080)?
    return Ok(stream)
}

async fn connect_websocket(url: read Url) -> Result<WebSocket, WebSocketError> {
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

    let check = check_package_dir(&root_dir).expect("package check should succeed");
    let input = package_lowering_input(&root_dir).expect("package should lower");
    let package = lower_sources_to_rust_package_with_options(
        &input.sources,
        &input.package.name,
        "/workspace/rsscript/runtime",
        &input.interfaces,
        &input.native_dependencies,
    )
    .expect("package source should lower with async provider");
    let _ = fs::remove_dir_all(&root_dir);

    assert!(check.ok, "{check:#?}");
    assert!(input.interfaces.iter().any(|(path, _)| {
        path.ends_with("packages/async/interface/timer.rssi")
            || path.ends_with("rss\\async\\interface\\timer.rssi")
    }));
    assert!(input.interfaces.iter().any(|(path, _)| {
        path.ends_with("packages/async/interface/file.rssi")
            || path.ends_with("rss\\async\\interface\\file.rssi")
    }));
    assert!(input.interfaces.iter().any(|(path, _)| {
        path.ends_with("packages/async/interface/http.rssi")
            || path.ends_with("rss\\async\\interface\\http.rssi")
    }));
    assert!(input.interfaces.iter().any(|(path, _)| {
        path.ends_with("packages/async/interface/process.rssi")
            || path.ends_with("rss\\async\\interface\\process.rssi")
    }));
    assert!(input.interfaces.iter().any(|(path, _)| {
        path.ends_with("packages/async/interface/tcp.rssi")
            || path.ends_with("rss\\async\\interface\\tcp.rssi")
    }));
    assert!(input.interfaces.iter().any(|(path, _)| {
        path.ends_with("packages/async/interface/websocket.rssi")
            || path.ends_with("rss\\async\\interface\\websocket.rssi")
    }));
    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::timer_sleep_native_start")
    );
    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::file_write_string_async")
    );
    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::file_read_all_string_async")
    );
    assert!(package.lib_rs.contains("rsscript_runtime::http_get_async"));
    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::process_run_stdout_async")
    );
    assert!(package.lib_rs.contains("rsscript_runtime::tcp_connect"));
    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::websocket_connect")
    );
    assert!(
        package
            .lib_rs
            .contains("rsscript_runtime::file_bytes_stream")
    );
    assert!(package.lib_rs.contains("rsscript_runtime::csv_rows"));
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

#[cfg(unix)]
#[test]
fn package_vendor_rejects_symlink_entries() {
    use std::os::unix::fs::symlink;

    let temp_dir = common::unique_temp_dir("rsscript-package-vendor-rejects-symlink");
    let root_dir = temp_dir.join("app");
    let dep_dir = temp_dir.join("dep");
    common::write_named_package_fixture(
        &dep_dir,
        "rss-vendor-symlink-dep",
        "0.1.0",
        "",
        r#"pub fn Dep.value() -> Int
"#,
    );
    let outside_file = temp_dir.join("outside.txt");
    fs::write(&outside_file, "secret").expect("outside file should be written");
    symlink(&outside_file, dep_dir.join("leak.txt")).expect("symlink should be created");
    common::write_named_package_fixture(
        &root_dir,
        "rss-vendor-symlink-app",
        "0.1.0",
        r#"[dependencies]
rss-vendor-symlink-dep = { path = "../dep" }
"#,
        r#"pub fn App.run() -> Int
"#,
    );

    let error = vendor_package_dir(&root_dir, false).expect_err("vendor should reject symlinks");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(error.contains("package copy rejects symlinks"));
    assert!(error.contains("leak.txt"));
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
fn package_review_marks_virtual_package_with_default() {
    let temp_dir = common::unique_temp_dir("rsscript-package-virtual-default");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-core-clock",
        "0.1.0",
        r#"[virtual]
has_default = true
"#,
        r#"pub fn Clock.now_ms() -> Int
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub fn Clock.now_ms() -> Int {
    return 0
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
    let tree = package_tree(&temp_dir).expect("package tree should succeed");
    let tree_json: Value = serde_json::from_str(&rsscript::format_package_tree_json(&tree))
        .expect("package tree JSON should parse");
    let _ = fs::remove_dir_all(&temp_dir);

    assert!(check.ok);
    assert_eq!(json["virtual_package"]["has_default"], true);
    assert_eq!(json["virtual_package"]["provider"], Value::Null);
    assert_eq!(tree_json["root"]["virtual_package"]["has_default"], true);
}

#[test]
fn package_check_rejects_invalid_virtual_package_shape() {
    let temp_dir = common::unique_temp_dir("rsscript-package-virtual-invalid");
    common::write_named_package_fixture(
        &temp_dir,
        "rss-core-clock",
        "0.1.0",
        r#"[virtual]
has_default = false
"#,
        r#"pub fn Clock.now_ms() -> Int
"#,
    );
    fs::create_dir_all(temp_dir.join("src")).expect("source dir should be created");
    fs::write(
        temp_dir.join("src/lib.rss"),
        r#"pub fn Clock.now_ms() -> Int {
    return 0
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
    assert_eq!(json["virtual_package"]["has_default"], false);
    assert!(json["diagnostics"].as_array().is_some_and(|diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "PKG0901"
                && diagnostic["label"] == "has_default"
                && diagnostic["summary"]
                    .as_str()
                    .is_some_and(|message| message.contains("without a default implementation"))
        })
    }));
}

#[test]
fn package_check_accepts_short_provider_alias_for_virtual_dependency() {
    let root_dir = common::unique_temp_dir("rsscript-package-short-provider-root");
    let interface_dir = common::unique_temp_dir("rsscript-package-short-provider-interface");
    let provider_dir = common::unique_temp_dir("rsscript-package-short-provider-impl");
    common::write_named_package_fixture(
        &interface_dir,
        "platform-env",
        "0.1.0",
        r#"[virtual]
has_default = false
provider = "env"
"#,
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
env = "posix-env"
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
    let tree = package_tree(&root_dir).expect("package tree should succeed");
    let tree_json: Value = serde_json::from_str(&rsscript::format_package_tree_json(&tree))
        .expect("package tree JSON should parse");
    let _ = fs::remove_dir_all(&root_dir);
    let _ = fs::remove_dir_all(&interface_dir);
    let _ = fs::remove_dir_all(&provider_dir);

    assert!(check.ok);
    assert_eq!(
        tree_json["root"]["dependencies"][0]["virtual_package"]["provider"],
        "env"
    );
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
fn package_metadata_fails_closed_on_error_diagnostics() {
    // A package with a parse error must not write authoritative review/REIR
    // artifacts that downstream gates would consume as evidence.
    let dir = common::unique_temp_dir("rsscript-package-metadata-failclosed");
    fs::create_dir_all(dir.join("interface")).expect("interface dir");
    fs::write(
        dir.join("rsspkg.toml"),
        "[package]\nname = \"bad\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interface\"]\n",
    )
    .expect("manifest");
    // Malformed declaration -> error diagnostic.
    fs::write(
        dir.join("interface/lib.rssi"),
        "native fn Broken.x(sql: read String -> String\n",
    )
    .expect("interface");

    let report = package_metadata(&dir, false).expect("metadata should produce a report");
    let reir_written = dir.join("review/reir/rsscript.json").exists();
    let _ = fs::remove_dir_all(&dir);

    assert!(!report.ok, "metadata of an erroring package must not be ok");
    assert!(
        !report.written,
        "authoritative artifacts must not be written"
    );
    assert!(
        !reir_written,
        "REIR bundle must not be written for an erroring package"
    );
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("refused to write")),
        "report should explain the fail-closed decision: {:?}",
        report.reasons
    );
}
