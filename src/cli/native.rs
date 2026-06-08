//! `rss native audit <package-dir>` — surface the security-relevant facts about
//! a package's native Rust adapter in one place: the heuristic source scan
//! (unsafe / ffi / filesystem / network / build script), the declared policies,
//! and the build inputs (Cargo.toml deps, Cargo.lock presence + hash, build.rs).
//!
//! Honest about its limits: the source scan is best-effort string matching and
//! transitive dependencies are NOT audited (reported as `not_audited`), so a
//! gate should treat an unaudited native adapter as high risk.

use std::path::Path;
use std::process::ExitCode;

use rsscript::review_package_dir;
use sha2::{Digest, Sha256};

pub(crate) fn run_native(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("audit") => run_native_audit(&args[1..]),
        _ => {
            eprintln!("usage: rss native audit [--json] <package-directory>");
            ExitCode::from(2)
        }
    }
}

fn run_native_audit(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut path = None;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other if !other.starts_with("--") => path = Some(other),
            other => {
                eprintln!("unknown native audit flag `{other}`");
                return ExitCode::from(2);
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: rss native audit [--json] <package-directory>");
        return ExitCode::from(2);
    };

    let review = match review_package_dir(Path::new(path)) {
        Ok(review) => review,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let Some(native) = review.native_rust else {
        eprintln!("package `{path}` declares no native Rust adapter to audit");
        return ExitCode::from(2);
    };

    let native_root = Path::new(path).join(&native.path);
    let cargo_toml = native_root.join("Cargo.toml");
    let cargo_lock = native_root.join("Cargo.lock");
    let build_rs = native_root.join("build.rs");

    let cargo_toml_text = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
    let declared_dependencies = declared_dependencies(&cargo_toml_text);
    let cargo_lock_text = std::fs::read_to_string(&cargo_lock).ok();
    let cargo_lock_sha256 = cargo_lock_text
        .as_ref()
        .map(|text| format!("sha256:{:x}", Sha256::digest(text.as_bytes())));

    let scan = &native.semantic.source_scan_best_effort;

    // Risk findings — anything unaudited or capability-touching is surfaced.
    let mut findings: Vec<String> = Vec::new();
    if scan.unsafe_detected {
        findings.push("unsafe Rust detected in native source (best-effort scan)".to_string());
    }
    if scan.ffi_detected {
        findings.push("FFI / extern detected in native source".to_string());
    }
    if scan.network_detected {
        findings.push("network access detected in native source".to_string());
    }
    if scan.filesystem_detected {
        findings.push("filesystem access detected in native source".to_string());
    }
    if scan.build_script_present {
        findings.push("build.rs present — runs arbitrary code at build time".to_string());
    }
    if cargo_lock_text.is_none() {
        findings.push("no Cargo.lock — native dependency versions are not pinned".to_string());
    }
    findings.push(
        "transitive dependencies are NOT audited (build scripts / proc macros / cfg-gated code \
         in deps are unknown); treat as high risk unless independently reviewed"
            .to_string(),
    );

    if json {
        let value = serde_json::json!({
            "$schema": "rsscript.native_audit.v0.1",
            "package_dir": path,
            "crate_name": native.crate_name,
            "native_path": native.path,
            "build_inputs": {
                "cargo_toml_present": !cargo_toml_text.is_empty(),
                "cargo_lock_present": cargo_lock_text.is_some(),
                "cargo_lock_sha256": cargo_lock_sha256,
                "build_rs_present": build_rs.exists(),
                "declared_dependencies": declared_dependencies,
                "transitive_dependencies": "not_audited",
            },
            "policy": {
                "build_scripts": native.build_scripts,
                "proc_macros": native.proc_macros,
                "unsafe": native.unsafe_policy,
                "native_links": native.native_links_policy,
                "ffi": native.ffi_policy,
            },
            "source_scan_best_effort": {
                "unsafe_detected": scan.unsafe_detected,
                "ffi_detected": scan.ffi_detected,
                "network_detected": scan.network_detected,
                "filesystem_detected": scan.filesystem_detected,
                "build_script_present": scan.build_script_present,
            },
            "findings": findings,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("native audit JSON serializes")
        );
    } else {
        println!(
            "native audit: {} (crate {})",
            path,
            native.crate_name.as_deref().unwrap_or("?")
        );
        println!("  path: {}", native.path);
        println!(
            "  build inputs: Cargo.toml={} Cargo.lock={} build.rs={} deps={}",
            !cargo_toml_text.is_empty(),
            cargo_lock_text.is_some(),
            build_rs.exists(),
            declared_dependencies.len()
        );
        if let Some(hash) = &cargo_lock_sha256 {
            println!("  Cargo.lock: {hash}");
        }
        println!(
            "  source scan (best-effort): unsafe={} ffi={} network={} filesystem={} build_script={}",
            scan.unsafe_detected,
            scan.ffi_detected,
            scan.network_detected,
            scan.filesystem_detected,
            scan.build_script_present,
        );
        println!("  findings:");
        for finding in &findings {
            println!("    - {finding}");
        }
    }

    ExitCode::SUCCESS
}

/// Dependency names declared in `[dependencies]` (best-effort TOML parse).
fn declared_dependencies(cargo_toml: &str) -> Vec<String> {
    let Ok(value) = cargo_toml.parse::<toml::Value>() else {
        return Vec::new();
    };
    value
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}
