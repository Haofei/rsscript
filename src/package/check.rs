use std::path::Path;

use super::graph::check_package_graph;
use super::lock::{
    compare_locked_packages, lock_package_dir, package_lock_diff_reasons, read_package_lock,
};
use super::native::check_package_native_rust;
use super::policy::{
    collect_manifest_review_policy_diagnostics, collect_manifest_review_policy_violations,
    package_review_policy_has_high_risk_violation, package_review_policy_ok,
};
use super::source_set::load_package;
use super::{
    PackageCheck, PackageCheckLock, PackageLock, PackageRisk, dedup_diagnostics, review_package_dir,
};

pub fn check_package_dir(package_dir: &Path) -> Result<PackageCheck, String> {
    let package = load_package(package_dir)?;
    let review = review_package_dir(package_dir)?;
    let current_lock = lock_package_dir(package_dir)?;
    let graph = check_package_graph(package_dir)?;
    let lock = check_package_lock(package_dir, &current_lock)?;
    let native_rust = check_package_native_rust(package_dir, review.native_rust.as_ref())?;

    let mut reasons = review.reasons.clone();
    reasons.extend(graph.reasons.clone());
    reasons.extend(lock.reasons.clone());
    if let Some(native) = &native_rust {
        reasons.extend(native.reasons.clone());
    }
    collect_manifest_review_policy_violations(
        &package.manifest,
        &review,
        native_rust.as_ref(),
        &mut reasons,
    );
    reasons.sort();
    reasons.dedup();

    let mut diagnostics = review.diagnostics.clone();
    diagnostics.extend(collect_manifest_review_policy_diagnostics(
        &package.manifest,
        package_dir,
        &review,
        native_rust.as_ref(),
        &package.sources,
    ));
    dedup_diagnostics(&mut diagnostics);
    let diagnostics_have_errors = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error());
    let native_ok = native_rust
        .as_ref()
        .is_none_or(|native_check| native_check.ok);
    let policy_ok = package_review_policy_ok(&package.manifest, &review, native_rust.as_ref());
    let ok = !diagnostics_have_errors && policy_ok && graph.ok && lock.matches && native_ok;
    let mut risk = review.risk.max(graph.risk).max(lock.risk);
    if let Some(native) = &native_rust {
        risk = risk.max(native.risk);
    }
    if diagnostics_have_errors {
        risk = risk.max(PackageRisk::High);
    }
    if package_review_policy_has_high_risk_violation(
        &package.manifest,
        &review,
        native_rust.as_ref(),
    ) {
        risk = risk.max(PackageRisk::High);
    }

    let mut summary = review.summary;
    summary.diagnostics = diagnostics.len();
    summary.errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
        .count();

    Ok(PackageCheck {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        ok,
        risk,
        reasons,
        virtual_package: review.virtual_package,
        implements: review.implements,
        summary,
        graph,
        lock,
        native_rust,
        diagnostics,
    })
}

fn check_package_lock(
    package_dir: &Path,
    current_lock: &PackageLock,
) -> Result<PackageCheckLock, String> {
    let lock_path = package_dir.join("rsspkg.lock");
    if !lock_path.exists() {
        return Ok(PackageCheckLock {
            path: lock_path.display().to_string(),
            present: false,
            matches: false,
            risk: PackageRisk::Elevated,
            reasons: vec!["rsspkg.lock missing".to_string()],
            package_changes: Vec::new(),
        });
    }

    let locked = read_package_lock(&lock_path)?;
    let package_changes = compare_locked_packages(&locked.packages, &current_lock.packages);
    let mut reasons = package_lock_diff_reasons(&package_changes);
    if locked.version != current_lock.version {
        reasons.push("lockfile format version changed".to_string());
    }
    reasons.sort();
    reasons.dedup();
    let mut risk = package_changes
        .iter()
        .fold(PackageRisk::Low, |risk, change| risk.max(change.risk));
    if locked.version != current_lock.version {
        risk = risk.max(PackageRisk::Elevated);
    }

    Ok(PackageCheckLock {
        path: lock_path.display().to_string(),
        present: true,
        matches: reasons.is_empty(),
        risk,
        reasons,
        package_changes,
    })
}
