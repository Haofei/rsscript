use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::diagnostic::code;
use crate::review::{ReviewFinding, ReviewRisk, review_sources};
use crate::syntax::ast::TypeKind;

use super::contract::{
    collect_package_function_contracts, collect_package_type_contracts,
    package_added_function_contract_is_high_risk, package_added_type_contract_is_high_risk,
    package_function_contract_boundary_changed, package_function_contracts_for_source,
    package_function_contracts_match, package_type_contract_boundary_changed,
    package_type_contracts_for_source, package_type_contracts_match,
};
use super::source_set::{ManifestNativeRust, PackageSource, load_package};
use super::{
    Manifest, PackageDiff, PackageInterfaceChange, PackageInterfaceChangeKind,
    PackageManifestChange, PackageReviewAwaitSite, PackageReviewFileKind, PackageRisk,
    feature_values_label, package_feature_may_change_boundary_risk, package_identity,
    package_risk_label, review_package_dir, toml_value_label,
};

pub fn diff_package_dirs(old_dir: &Path, new_dir: &Path) -> Result<PackageDiff, String> {
    let old_package = load_package(old_dir)?;
    let new_package = load_package(new_dir)?;
    let old_review = review_package_dir(old_dir)?;
    let new_review = review_package_dir(new_dir)?;

    let mut manifest_changes = Vec::new();
    compare_package_identity(
        &old_package.manifest,
        &new_package.manifest,
        &mut manifest_changes,
    );
    compare_value_maps(
        "dependency",
        &old_package.manifest.dependencies,
        &new_package.manifest.dependencies,
        PackageRisk::High,
        &mut manifest_changes,
    );
    compare_value_maps(
        "dev-dependency",
        &old_package.manifest.dev_dependencies,
        &new_package.manifest.dev_dependencies,
        PackageRisk::Elevated,
        &mut manifest_changes,
    );
    compare_feature_maps(
        &old_package.manifest.features,
        &new_package.manifest.features,
        &mut manifest_changes,
    );
    compare_native_rust(
        old_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
        new_package
            .manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
        &mut manifest_changes,
    );

    let interface_changes = compare_interface_sources(&old_package.sources, &new_package.sources);
    let await_sites_changed = package_await_sites_changed(
        old_review.await_sites.as_slice(),
        new_review.await_sites.as_slice(),
    );
    let mut reasons = package_diff_reasons(&manifest_changes, &interface_changes);
    if await_sites_changed {
        reasons.push("await site review metadata changed".to_string());
    }
    if old_review.risk != new_review.risk {
        reasons.push(format!(
            "package risk changed from {} to {}",
            package_risk_label(old_review.risk),
            package_risk_label(new_review.risk)
        ));
    }
    reasons.sort();
    reasons.dedup();
    let risk = package_diff_risk(
        &manifest_changes,
        &interface_changes,
        old_review.risk,
        new_review.risk,
        await_sites_changed,
    );

    Ok(PackageDiff {
        old_package: package_identity(&old_package.manifest),
        new_package: package_identity(&new_package.manifest),
        risk,
        reasons,
        manifest_changes,
        interface_changes,
        old_review: old_review.summary,
        new_review: new_review.summary,
    })
}

fn compare_package_identity(
    old: &Manifest,
    new: &Manifest,
    changes: &mut Vec<PackageManifestChange>,
) {
    if old.package.name != new.package.name {
        changes.push(manifest_change(
            "package",
            "name",
            Some(old.package.name.clone()),
            Some(new.package.name.clone()),
            PackageRisk::High,
        ));
    }
    if old.package.version != new.package.version {
        changes.push(manifest_change(
            "package",
            "version",
            Some(old.package.version.clone()),
            Some(new.package.version.clone()),
            PackageRisk::Elevated,
        ));
    }
    if old.package.edition != new.package.edition {
        changes.push(manifest_change(
            "package",
            "edition",
            Some(old.package.edition.clone()),
            Some(new.package.edition.clone()),
            PackageRisk::Elevated,
        ));
    }
}

fn compare_value_maps(
    kind: &str,
    old: &BTreeMap<String, toml::Value>,
    new: &BTreeMap<String, toml::Value>,
    risk: PackageRisk,
    changes: &mut Vec<PackageManifestChange>,
) {
    let names = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.get(&name).map(toml_value_label);
        let after = new.get(&name).map(toml_value_label);
        if before != after {
            changes.push(manifest_change(kind, name, before, after, risk));
        }
    }
}

fn compare_feature_maps(
    old: &BTreeMap<String, Vec<String>>,
    new: &BTreeMap<String, Vec<String>>,
    changes: &mut Vec<PackageManifestChange>,
) {
    let names = old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.get(&name).map(|values| feature_values_label(values));
        let after = new.get(&name).map(|values| feature_values_label(values));
        if before != after {
            let risk = new
                .get(&name)
                .or_else(|| old.get(&name))
                .map_or(PackageRisk::Elevated, |values| {
                    package_feature_risk(&name, values)
                });
            changes.push(manifest_change(
                "package-feature",
                name,
                before,
                after,
                risk,
            ));
        }
    }
}

fn package_feature_risk(name: &str, values: &[String]) -> PackageRisk {
    if package_feature_may_change_boundary_risk(name, values) {
        PackageRisk::High
    } else {
        PackageRisk::Elevated
    }
}

fn compare_native_rust(
    old: Option<&ManifestNativeRust>,
    new: Option<&ManifestNativeRust>,
    changes: &mut Vec<PackageManifestChange>,
) {
    let old_enabled = old.is_some_and(|native| native.enabled);
    let new_enabled = new.is_some_and(|native| native.enabled);
    if old_enabled != new_enabled {
        changes.push(manifest_change(
            "native-rust",
            "enabled",
            Some(old_enabled.to_string()),
            Some(new_enabled.to_string()),
            PackageRisk::High,
        ));
    }
    compare_optional_native_field(
        "path",
        old.and_then(|native| native.path.as_deref()),
        new.and_then(|native| native.path.as_deref()),
        PackageRisk::Elevated,
        changes,
    );
    compare_optional_native_field(
        "crate",
        old.and_then(|native| native.crate_name.as_deref()),
        new.and_then(|native| native.crate_name.as_deref()),
        PackageRisk::Elevated,
        changes,
    );
    let old_cargo_features = old.map(|native| feature_values_label(&native.cargo_features));
    let new_cargo_features = new.map(|native| feature_values_label(&native.cargo_features));
    if old_cargo_features != new_cargo_features {
        changes.push(manifest_change(
            "native-rust",
            "cargo_features",
            old_cargo_features,
            new_cargo_features,
            PackageRisk::Elevated,
        ));
    }
    let old_feature_map = old.map(native_feature_map_label);
    let new_feature_map = new.map(native_feature_map_label);
    if old_feature_map != new_feature_map {
        changes.push(manifest_change(
            "native-rust",
            "feature_map",
            old_feature_map,
            new_feature_map,
            PackageRisk::Elevated,
        ));
    }
    compare_optional_native_field(
        "build_scripts",
        old.and_then(|native| native.effective_build_scripts()),
        new.and_then(|native| native.effective_build_scripts()),
        PackageRisk::High,
        changes,
    );
    compare_optional_native_field(
        "proc_macros",
        old.and_then(|native| native.effective_proc_macros()),
        new.and_then(|native| native.effective_proc_macros()),
        PackageRisk::High,
        changes,
    );
    compare_optional_native_field(
        "unsafe",
        old.and_then(|native| native.effective_unsafe_policy()),
        new.and_then(|native| native.effective_unsafe_policy()),
        PackageRisk::High,
        changes,
    );
    let old_links = old.map(|native| native.links.join(", "));
    let new_links = new.map(|native| native.links.join(", "));
    if old_links != new_links {
        changes.push(manifest_change(
            "native-rust",
            "links",
            old_links,
            new_links,
            PackageRisk::High,
        ));
    }
}

fn native_feature_map_label(native: &ManifestNativeRust) -> String {
    native
        .feature_map
        .iter()
        .map(|(feature, mapping)| {
            format!(
                "{}:{}",
                feature,
                feature_values_label(&mapping.cargo_features)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn compare_optional_native_field(
    name: &str,
    old: Option<&str>,
    new: Option<&str>,
    risk: PackageRisk,
    changes: &mut Vec<PackageManifestChange>,
) {
    if old != new {
        changes.push(manifest_change(
            "native-rust",
            name,
            old.map(str::to_string),
            new.map(str::to_string),
            risk,
        ));
    }
}

fn compare_interface_sources(
    old_sources: &[PackageSource],
    new_sources: &[PackageSource],
) -> Vec<PackageInterfaceChange> {
    let old_interfaces = interface_sources_by_relative_path(old_sources);
    let new_interfaces = interface_sources_by_relative_path(new_sources);
    let files = old_interfaces
        .keys()
        .chain(new_interfaces.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for file in files {
        match (old_interfaces.get(&file), new_interfaces.get(&file)) {
            (Some(old), Some(new)) if old.contents != new.contents => {
                let findings = review_sources(&old.path, &old.contents, &new.path, &new.contents);
                let risk = modified_interface_change_risk(old, new, new_sources, &findings);
                changes.push(PackageInterfaceChange {
                    file,
                    change: PackageInterfaceChangeKind::Modified,
                    risk,
                    findings,
                });
            }
            (None, Some(_)) => {
                let risk = added_interface_change_risk(new_sources, &file);
                changes.push(PackageInterfaceChange {
                    file,
                    change: PackageInterfaceChangeKind::Added,
                    risk,
                    findings: Vec::new(),
                });
            }
            (Some(_), None) => changes.push(PackageInterfaceChange {
                file,
                change: PackageInterfaceChangeKind::Removed,
                risk: PackageRisk::High,
                findings: Vec::new(),
            }),
            _ => {}
        }
    }
    changes
}

fn interface_sources_by_relative_path(
    sources: &[PackageSource],
) -> BTreeMap<String, &PackageSource> {
    sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.relative_path.clone(), source))
        .collect()
}

fn package_diff_reasons(
    manifest_changes: &[PackageManifestChange],
    interface_changes: &[PackageInterfaceChange],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if manifest_changes
        .iter()
        .any(|change| change.kind == "dependency")
    {
        reasons.push("RSScript dependencies changed".to_string());
    }
    if manifest_changes
        .iter()
        .any(|change| change.kind == "package-feature")
    {
        reasons.push("package features changed".to_string());
    }
    if manifest_changes
        .iter()
        .any(|change| change.kind == "native-rust")
    {
        reasons.push("native Rust wrapper metadata changed".to_string());
    }
    if !interface_changes.is_empty() {
        reasons.push("public .rssi semantic contract changed".to_string());
    }
    if interface_changes
        .iter()
        .any(|change| change.risk == PackageRisk::High)
    {
        reasons.push("high-risk interface change detected".to_string());
    }
    reasons
}

fn package_diff_risk(
    manifest_changes: &[PackageManifestChange],
    interface_changes: &[PackageInterfaceChange],
    old_risk: PackageRisk,
    new_risk: PackageRisk,
    await_sites_changed: bool,
) -> PackageRisk {
    let mut risk = old_risk.max(new_risk);
    for change in manifest_changes {
        risk = risk.max(change.risk);
    }
    for change in interface_changes {
        risk = risk.max(change.risk);
    }
    if await_sites_changed {
        risk = risk.max(PackageRisk::Elevated);
    }
    risk
}

fn package_await_sites_changed(
    old_sites: &[PackageReviewAwaitSite],
    new_sites: &[PackageReviewAwaitSite],
) -> bool {
    package_await_site_labels(old_sites) != package_await_site_labels(new_sites)
}

fn package_await_site_labels(sites: &[PackageReviewAwaitSite]) -> Vec<String> {
    let mut labels = sites
        .iter()
        .map(package_await_site_label)
        .collect::<Vec<_>>();
    labels.sort();
    labels
}

fn package_await_site_label(site: &PackageReviewAwaitSite) -> String {
    format!(
        "{}|{}|{}|{}",
        site.function,
        site.callee.as_deref().unwrap_or("<unknown>"),
        site.span.file,
        site.live_across_await.join(",")
    )
}

fn interface_change_risk(findings: &[ReviewFinding]) -> PackageRisk {
    if findings
        .iter()
        .any(|finding| finding.code == code::REVIEW_PROTOCOL_IMPL_CHANGED)
    {
        return PackageRisk::High;
    }
    if findings.iter().any(|finding| {
        matches!(
            finding.risk,
            ReviewRisk::Unsafe | ReviewRisk::Effect | ReviewRisk::Boundary | ReviewRisk::Guarantee
        )
    }) {
        PackageRisk::High
    } else if findings.is_empty() {
        PackageRisk::Low
    } else {
        PackageRisk::Elevated
    }
}

fn modified_interface_change_risk(
    old: &PackageSource,
    new: &PackageSource,
    new_sources: &[PackageSource],
    findings: &[ReviewFinding],
) -> PackageRisk {
    interface_change_risk(findings).max(modified_contracts_in_interface_risk(old, new, new_sources))
}

fn added_interface_change_risk(new_sources: &[PackageSource], file: &str) -> PackageRisk {
    let Some(source_path) = new_sources
        .iter()
        .find(|source| {
            source.kind == PackageReviewFileKind::Interface && source.relative_path == file
        })
        .map(|source| source.path.as_str())
    else {
        return PackageRisk::Low;
    };
    let type_contracts =
        collect_package_type_contracts(new_sources, PackageReviewFileKind::Interface);
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut saw_contract = false;

    for contract in type_contracts.values() {
        if contract.span.file != source_path {
            continue;
        }
        saw_contract = true;
        if contract.kind == TypeKind::Resource
            || contract
                .fields
                .iter()
                .any(|field| field.is_handle || field.is_weak)
        {
            return PackageRisk::High;
        }
    }

    let function_contracts =
        collect_package_function_contracts(new_sources, PackageReviewFileKind::Interface);
    for contract in function_contracts.values() {
        if contract.span.file != source_path {
            continue;
        }
        saw_contract = true;
        if package_added_function_contract_is_high_risk(contract, &resource_types) {
            return PackageRisk::High;
        }
    }

    if saw_contract {
        PackageRisk::Elevated
    } else {
        PackageRisk::Low
    }
}

fn modified_contracts_in_interface_risk(
    old: &PackageSource,
    new: &PackageSource,
    new_sources: &[PackageSource],
) -> PackageRisk {
    let old_type_contracts = package_type_contracts_for_source(old);
    let new_type_contracts = package_type_contracts_for_source(new);
    let old_function_contracts = package_function_contracts_for_source(old);
    let new_function_contracts = package_function_contracts_for_source(new);
    let mut resource_types =
        collect_package_type_contracts(new_sources, PackageReviewFileKind::Interface)
            .values()
            .filter(|contract| contract.kind == TypeKind::Resource)
            .map(|contract| contract.name.clone())
            .collect::<BTreeSet<_>>();
    resource_types.extend(
        old_type_contracts
            .values()
            .filter(|contract| contract.kind == TypeKind::Resource)
            .map(|contract| contract.name.clone()),
    );
    let resource_type_refs = resource_types
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut risk = PackageRisk::Low;

    for name in old_type_contracts.keys() {
        if !new_type_contracts.contains_key(name) {
            return PackageRisk::High;
        }
    }

    for name in old_function_contracts.keys() {
        if !new_function_contracts.contains_key(name) {
            return PackageRisk::High;
        }
    }

    for (name, contract) in &new_type_contracts {
        match old_type_contracts.get(name) {
            Some(old_contract) => {
                if package_type_contracts_match(old_contract, contract) {
                    continue;
                }
                if package_type_contract_boundary_changed(old_contract, contract) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::High);
            }
            None => {
                if package_added_type_contract_is_high_risk(contract) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::Elevated);
            }
        }
    }

    for (name, contract) in &new_function_contracts {
        match old_function_contracts.get(name) {
            Some(old_contract) => {
                if package_function_contracts_match(old_contract, contract) {
                    continue;
                }
                if package_function_contract_boundary_changed(
                    old_contract,
                    contract,
                    &resource_type_refs,
                ) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::Elevated);
            }
            None => {
                if package_added_function_contract_is_high_risk(contract, &resource_type_refs) {
                    return PackageRisk::High;
                }
                risk = risk.max(PackageRisk::Elevated);
            }
        }
    }

    risk
}

fn manifest_change(
    kind: impl Into<String>,
    name: impl Into<String>,
    before: Option<String>,
    after: Option<String>,
    risk: PackageRisk,
) -> PackageManifestChange {
    PackageManifestChange {
        kind: kind.into(),
        name: name.into(),
        before,
        after,
        risk,
    }
}
