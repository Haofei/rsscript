use std::path::Path;

use rsscript_diagnostics::{Diagnostic, Span, code};
use rsscript_project::read_project_utf8_file_bounded;

use crate::ManifestReviewPolicy;
use crate::collect_package_function_contracts;
use crate::{Manifest, PackageSource};
use rsscript_package_model::{
    PackageNativeRustCheck, PackageReview, PackageReviewFileKind, PackageRisk,
};

const PACKAGE_MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

fn manifest_native_enabled(manifest: &Manifest) -> bool {
    manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .is_some_and(|native| native.enabled)
}

fn manifest_native_unsafe_boundary(manifest: &Manifest) -> bool {
    manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .is_some_and(|native| {
            native
                .effective_unsafe_policies()
                .has_non_forbidden_boundary()
        })
}

fn package_manifest_key_span(package_dir: &Path, key: &str) -> Span {
    let path = package_dir.join("rsspkg.toml");
    let source = read_project_utf8_file_bounded(
        &path,
        PACKAGE_MANIFEST_MAX_BYTES,
        "package review policy diagnostic read",
    )
    .unwrap_or_default();
    for (index, line) in source.lines().enumerate() {
        if let Some(column) = line.find(key) {
            return Span {
                file: path.display().to_string(),
                line: index + 1,
                column: column + 1,
                length: key.len().max(1),
            };
        }
    }
    Span {
        file: path.display().to_string(),
        line: 1,
        column: 1,
        length: key.len().max(1),
    }
}

pub fn package_review_policy_ok(
    manifest: &Manifest,
    review: &PackageReview,
    native_check: Option<&PackageNativeRustCheck>,
) -> bool {
    let Some(review_policy) = manifest.review.as_ref() else {
        return true;
    };
    let policy = &review_policy.policy;
    if policy.deny_unknown == Some(true) && review.risk == PackageRisk::Unknown {
        return false;
    }
    if policy.deny_native == Some(true)
        && (review.summary.native_apis > 0
            || manifest_native_enabled(manifest)
            || native_check.is_some_and(|native| native.risk >= PackageRisk::Elevated))
    {
        return false;
    }
    if policy.deny_unsafe_apis == Some(true)
        && (review.summary.unsafe_apis > 0
            || manifest_native_unsafe_boundary(manifest)
            || native_check.is_some_and(|native| native.unsafe_detected))
    {
        return false;
    }
    true
}

pub fn package_review_policy_has_high_risk_violation(
    manifest: &Manifest,
    review: &PackageReview,
    native_check: Option<&PackageNativeRustCheck>,
) -> bool {
    let Some(review_policy) = manifest.review.as_ref() else {
        return false;
    };
    let policy = &review_policy.policy;
    policy.deny_native == Some(true)
        && (review.summary.native_apis > 0
            || manifest_native_enabled(manifest)
            || native_check.is_some_and(|native| native.risk >= PackageRisk::Elevated))
        || policy.deny_unsafe_apis == Some(true)
            && (review.summary.unsafe_apis > 0
                || manifest_native_unsafe_boundary(manifest)
                || native_check.is_some_and(|native| native.unsafe_detected))
}

pub fn collect_manifest_review_policy_diagnostics(
    manifest: &Manifest,
    package_dir: &Path,
    review: &PackageReview,
    native_check: Option<&PackageNativeRustCheck>,
    sources: &[PackageSource],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(native_rust_policy_value_diagnostics(package_dir, manifest));
    let Some(review_policy) = manifest.review.as_ref() else {
        return diagnostics;
    };
    let policy = &review_policy.policy;
    if policy.deny_unknown == Some(true) && review.risk == PackageRisk::Unknown {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_unknown",
            "package review policy denies unknown review risk.",
            "deny_unknown",
            "The package computed unknown review risk while `[review.policy].deny_unknown = true`.",
        ));
    }
    if policy.deny_native == Some(true) && review.summary.native_apis > 0 {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_native",
            "package review policy denies native public APIs.",
            "deny_native",
            "The public `.rssi` contract exposes native APIs while `[review.policy].deny_native = true`.",
        ));
    }
    if policy.deny_native == Some(true) && manifest_native_enabled(manifest) {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_native",
            "package review policy denies native Rust wrappers.",
            "deny_native",
            "The manifest enables a native Rust wrapper while `[review.policy].deny_native = true`.",
        ));
    }
    if policy.deny_native == Some(true)
        && native_check.is_some_and(|native| native.risk >= PackageRisk::Elevated)
    {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_native",
            "package review policy denies native implementation risk.",
            "deny_native",
            "Native Rust metadata raised package risk while `[review.policy].deny_native = true`.",
        ));
    }
    if policy.deny_unsafe_apis == Some(true) && review.summary.unsafe_apis > 0 {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_unsafe_apis",
            "package review policy denies unsafe public APIs.",
            "deny_unsafe_apis",
            "The public `.rssi` contract exposes unsafe APIs while `[review.policy].deny_unsafe_apis = true`.",
        ));
    }
    if policy.deny_unsafe_apis == Some(true) && manifest_native_unsafe_boundary(manifest) {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_unsafe_apis",
            "package review policy denies native unsafe policy.",
            "deny_unsafe_apis",
            "The native Rust policy permits unsafe boundaries while `[review.policy].deny_unsafe_apis = true`.",
        ));
    }
    if policy.deny_unsafe_apis == Some(true)
        && native_check.is_some_and(|native| native.unsafe_detected)
    {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "deny_unsafe_apis",
            "package review policy denies detected unsafe native code.",
            "deny_unsafe_apis",
            "Native Rust code contains unsafe blocks while `[review.policy].deny_unsafe_apis = true`.",
        ));
    }
    diagnostics.extend(package_signature_policy_diagnostics(
        package_dir,
        policy,
        sources,
    ));
    diagnostics.extend(package_review_policy_value_diagnostics(package_dir, policy));
    diagnostics
}

fn native_rust_policy_value_diagnostics(
    package_dir: &Path,
    manifest: &Manifest,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(native) = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
    else {
        return diagnostics;
    };

    for (key, value) in [
        ("build_scripts", native.policy.build_scripts.as_deref()),
        ("proc_macros", native.policy.proc_macros.as_deref()),
        ("native_links", native.policy.native_links.as_deref()),
        ("ffi", native.policy.ffi.as_deref()),
        ("rss_unsafe_apis", native.policy.rss_unsafe_apis.as_deref()),
        (
            "wrapper_unsafe_blocks",
            native.policy.wrapper_unsafe_blocks.as_deref(),
        ),
        (
            "transitive_unsafe_blocks",
            native.policy.transitive_unsafe_blocks.as_deref(),
        ),
    ] {
        let Some(value) = value else {
            continue;
        };
        if !matches!(value, "forbid" | "review" | "allow") {
            diagnostics.push(package_review_policy_diagnostic(
                package_dir,
                key,
                format!("native Rust policy has invalid `{key}`."),
                key,
                format!("`{key}` must be `forbid`, `review`, or `allow`."),
            ));
        }
    }
    diagnostics
}

fn package_signature_policy_diagnostics(
    package_dir: &Path,
    policy: &ManifestReviewPolicy,
    sources: &[PackageSource],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let interface_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let source_contracts;
    let contracts = if interface_contracts.is_empty() {
        source_contracts =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_contracts
    } else {
        &interface_contracts
    };

    if let Some(limit) = policy.max_public_params {
        for contract in contracts
            .values()
            .filter(|contract| contract.params.len() > limit)
        {
            diagnostics.push(package_review_policy_diagnostic(
                package_dir,
                "max_public_params",
                format!("package review policy limits public signatures to {limit} parameters."),
                "max_public_params",
                format!(
                    "Public API `{}` has {} parameters.",
                    contract.name,
                    contract.params.len()
                ),
            ));
        }
    }

    if let Some(limit) = policy.max_nested_type_depth {
        for contract in contracts.values() {
            for param in &contract.params {
                let depth = package_type_name_depth(&param.type_name);
                if depth > limit {
                    diagnostics.push(package_review_policy_diagnostic(
                        package_dir,
                        "max_nested_type_depth",
                        format!(
                            "package review policy limits public type nesting to depth {limit}."
                        ),
                        "max_nested_type_depth",
                        format!(
                            "Parameter `{}` in public API `{}` has nested type depth {depth}.",
                            param.name, contract.name
                        ),
                    ));
                }
            }
            if let Some(return_type) = &contract.return_type {
                let depth = package_type_name_depth(return_type);
                if depth > limit {
                    diagnostics.push(package_review_policy_diagnostic(
                        package_dir,
                        "max_nested_type_depth",
                        format!(
                            "package review policy limits public type nesting to depth {limit}."
                        ),
                        "max_nested_type_depth",
                        format!(
                            "Return type of public API `{}` has nested type depth {depth}.",
                            contract.name
                        ),
                    ));
                }
            }
        }
    }

    diagnostics
}

fn package_review_policy_value_diagnostics(
    package_dir: &Path,
    policy: &ManifestReviewPolicy,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(value) = policy.native_api_risk.as_deref()
        && !matches!(value, "elevated" | "high")
    {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "native_api_risk",
            "package review policy has invalid `native_api_risk`.",
            "native_api_risk",
            "`native_api_risk` must be `elevated` or `high`.",
        ));
    }
    if let Some(value) = policy.build_execution_default.as_deref()
        && !matches!(value, "forbid" | "review" | "allow")
    {
        diagnostics.push(package_review_policy_diagnostic(
            package_dir,
            "build_execution_default",
            "package review policy has invalid `build_execution_default`.",
            "build_execution_default",
            "`build_execution_default` must be `forbid`, `review`, or `allow`.",
        ));
    }
    diagnostics
}

fn package_type_name_depth(type_name: &str) -> usize {
    let mut depth: usize = 1;
    let mut max_depth: usize = 1;
    for character in type_name.chars() {
        match character {
            '<' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            '>' => {
                depth = depth.saturating_sub(1).max(1);
            }
            _ => {}
        }
    }
    max_depth
}

fn package_review_policy_diagnostic(
    package_dir: &Path,
    key: &str,
    summary: impl Into<String>,
    label: impl Into<String>,
    cause: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_REVIEW_POLICY_VIOLATION,
        summary,
        package_manifest_key_span(package_dir, key),
        label,
    )
    .with_cause(cause)
    .with_fix(
        "update_review_policy",
        "Relax the package review policy, or remove the API/risk source that violates it.",
        "manual",
    )
}

pub fn collect_manifest_review_policy_violations(
    manifest: &Manifest,
    review: &PackageReview,
    native_check: Option<&PackageNativeRustCheck>,
    reasons: &mut Vec<String>,
) {
    let Some(review_policy) = manifest.review.as_ref() else {
        return;
    };
    let policy = &review_policy.policy;
    if policy.deny_unknown == Some(true) && review.risk == PackageRisk::Unknown {
        reasons.push("package policy denies unknown review risk".to_string());
    }
    if policy.deny_native == Some(true) && review.summary.native_apis > 0 {
        reasons.push("package policy denies native public APIs".to_string());
    }
    if policy.deny_native == Some(true) && manifest_native_enabled(manifest) {
        reasons.push("package policy denies native Rust wrappers".to_string());
    }
    if policy.deny_native == Some(true)
        && native_check.is_some_and(|native| native.risk >= PackageRisk::Elevated)
    {
        reasons.push("package policy denies native implementation risk".to_string());
    }
    if policy.deny_unsafe_apis == Some(true) && review.summary.unsafe_apis > 0 {
        reasons.push("package policy denies unsafe public APIs".to_string());
    }
    if policy.deny_unsafe_apis == Some(true) && manifest_native_unsafe_boundary(manifest) {
        reasons.push("package policy denies native unsafe policy".to_string());
    }
    if policy.deny_unsafe_apis == Some(true)
        && native_check.is_some_and(|native| native.unsafe_detected)
    {
        reasons.push("package policy denies detected unsafe native code".to_string());
    }
}
