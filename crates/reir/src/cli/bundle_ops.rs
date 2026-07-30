use super::CliError;
use super::safe_io::{MAX_CLI_INPUT_BYTES, read_bounded_text_accounted};
use reir::adapters::rsscript::{
    rsscript_check_json_to_bundle, rsscript_json_to_bundle, rsscript_lock_diff_json_to_bundle,
    rsscript_lock_json_to_bundle, rsscript_metadata_json_to_bundle, rsscript_tree_json_to_bundle,
};
use reir::api::v1::{
    model::{Bundle, FactRole},
    reconciliation::{
        Diff, Reconciliation, ReconciliationStatus, Slice, reconcile_capabilities_for_target,
        slice_by_kind,
    },
};
use std::collections::BTreeMap;
use std::process::ExitCode;

pub(super) const MAX_MERGE_INPUT_FILES: usize = 1024;

pub(super) struct RsscriptCollectInputs<'a> {
    pub(super) review_map_json: Option<&'a str>,
    pub(super) package_review_json: Option<&'a str>,
    pub(super) package_check_json: Option<&'a str>,
    pub(super) package_lock_json: Option<&'a str>,
    pub(super) package_lock_path: Option<&'a str>,
    pub(super) lock_update_json: Option<&'a str>,
    pub(super) package_tree_json: Option<&'a str>,
    pub(super) package_metadata_json: Option<&'a str>,
    pub(super) package_name: Option<&'a str>,
}

pub(super) fn collect_rsscript_bundle(
    inputs: RsscriptCollectInputs<'_>,
) -> Result<Bundle, CliError> {
    let mut bundles = Vec::new();
    if inputs.review_map_json.is_some() || inputs.package_review_json.is_some() {
        bundles.push(
            rsscript_json_to_bundle(
                inputs.review_map_json,
                inputs.package_review_json,
                inputs.package_name,
            )
            .map_err(|error| {
                CliError::runtime(format!(
                    "failed to collect RSScript review evidence: {error}"
                ))
            })?,
        );
    }
    if let Some(json) = inputs.package_check_json {
        bundles.push(rsscript_check_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript check evidence: {error}"
            ))
        })?);
    }
    if let Some(json) = inputs.package_lock_json {
        let lock_json = package_lock_json_with_path(json, inputs.package_lock_path)?;
        bundles.push(rsscript_lock_json_to_bundle(&lock_json).map_err(|error| {
            CliError::runtime(format!("failed to collect RSScript lock evidence: {error}"))
        })?);
    }
    if let Some(json) = inputs.lock_update_json {
        bundles.push(rsscript_lock_diff_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript lock-update evidence: {error}"
            ))
        })?);
    }
    if let Some(json) = inputs.package_tree_json {
        bundles.push(rsscript_tree_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!("failed to collect RSScript tree evidence: {error}"))
        })?);
    }
    if let Some(json) = inputs.package_metadata_json {
        bundles.push(rsscript_metadata_json_to_bundle(json).map_err(|error| {
            CliError::runtime(format!(
                "failed to collect RSScript metadata evidence: {error}"
            ))
        })?);
    }
    if bundles.is_empty() {
        return Err(CliError::usage(
            "collect requires at least one RSScript JSON input",
        ));
    }
    merge_bundle_values(bundles)
}

pub(super) fn package_lock_json_with_path(
    json: &str,
    path: Option<&str>,
) -> Result<String, CliError> {
    let Some(path) = path else {
        return Ok(json.to_owned());
    };
    let mut value: serde_json::Value = serde_json::from_str(json).map_err(|error| {
        CliError::runtime(format!("failed to parse RSScript lock evidence: {error}"))
    })?;
    if value
        .get("lockfile_path")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        value
            .as_object_mut()
            .ok_or_else(|| CliError::runtime("RSScript lock evidence must be a JSON object"))?
            .insert("lockfile_path".to_owned(), path.to_owned().into());
    }
    serde_json::to_string(&value).map_err(|error| {
        CliError::runtime(format!(
            "failed to prepare RSScript lock evidence with path: {error}"
        ))
    })
}

pub(super) fn merge_bundles(paths: &[String]) -> Result<Bundle, CliError> {
    if paths.len() > MAX_MERGE_INPUT_FILES {
        return Err(CliError::runtime(format!(
            "merge accepts at most {MAX_MERGE_INPUT_FILES} input files"
        )));
    }
    let mut aggregate_bytes = 0;
    let bundles = paths
        .iter()
        .map(|path| {
            let json =
                read_bounded_text_accounted(path, &mut aggregate_bytes, MAX_CLI_INPUT_BYTES)?;
            Bundle::from_json(&json)
                .map_err(|error| CliError::runtime(format!("failed to parse {path}: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    merge_bundle_values(bundles)
}

pub(super) fn merge_bundle_values(bundles: Vec<Bundle>) -> Result<Bundle, CliError> {
    let mut bundles = bundles.into_iter();
    let mut merged = bundles.next().unwrap_or_default();
    let mut input_slices = merged.slices.clone();
    for bundle in bundles {
        if bundle.schema != merged.schema {
            return Err(CliError::runtime(format!(
                "cannot merge bundle with schema {} into {}",
                bundle.schema, merged.schema
            )));
        }
        if bundle.ontology != merged.ontology {
            return Err(CliError::runtime(format!(
                "cannot merge bundle with ontology {} into {}",
                bundle.ontology, merged.ontology
            )));
        }
        merged.producers.extend(bundle.producers);
        merged.subjects.extend(bundle.subjects);
        merged.subject_chains.extend(bundle.subject_chains);
        merged.facts.extend(bundle.facts);
        merged.edges.extend(bundle.edges);
        merged.reconciliations.extend(bundle.reconciliations);
        input_slices.extend(bundle.slices);
        merged.policy_results.extend(bundle.policy_results);
        merged.profiles.extend(bundle.profiles);
        merged.diffs.extend(bundle.diffs);
        merged.exceptions.extend(bundle.exceptions);
    }

    dedupe_bundle(&mut merged);
    rebuild_subject_index(&mut merged);
    merged.slices = merged_slices(input_slices, &merged);
    Ok(merged)
}

pub(super) fn reconcile_bundle(bundle: &mut Bundle, target: Option<&str>) {
    let required = bundle
        .facts
        .iter()
        .filter(|fact| fact.role == Some(FactRole::Required))
        .cloned()
        .collect::<Vec<_>>();
    let granted = bundle
        .facts
        .iter()
        .filter(|fact| fact.role == Some(FactRole::Granted))
        .cloned()
        .collect::<Vec<_>>();
    bundle.reconciliations = reconcile_capabilities_for_target(&required, &granted, target);
    bundle.slices = slice_by_kind(bundle);
}

pub(super) fn exit_for_reconciliations(reconciliations: &[Reconciliation]) -> ExitCode {
    if reconciliations
        .iter()
        .any(|reconciliation| reconciliation.status == ReconciliationStatus::Fail)
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

pub(super) fn exit_for_diff(diff: &Diff, fail_on_change: bool) -> ExitCode {
    if fail_on_change && !diff.items.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

pub(super) fn dedupe_bundle(bundle: &mut Bundle) {
    dedupe_by_key(&mut bundle.producers, producer_key);
    dedupe_by_key(&mut bundle.subject_chains, |chain| chain.id.clone());
    dedupe_by_key(&mut bundle.facts, |fact| fact.id.clone());
    dedupe_by_key(&mut bundle.edges, |edge| edge.id.clone());
    dedupe_by_key(&mut bundle.reconciliations, |reconciliation| {
        reconciliation.id.clone()
    });
    dedupe_by_key(&mut bundle.policy_results, |result| result.id.clone());
    dedupe_by_key(&mut bundle.profiles, |profile| profile.kind.clone());
    dedupe_by_key(&mut bundle.diffs, |diff| diff.id.clone());
    dedupe_by_key(&mut bundle.exceptions, |exception| exception.id.clone());
}

pub(super) fn rebuild_subject_index(bundle: &mut Bundle) {
    let mut subjects = BTreeMap::new();
    for subject in &bundle.subjects {
        subjects.insert(subject.id.clone(), subject.clone());
    }
    for fact in &bundle.facts {
        subjects.insert(fact.subject.id.clone(), fact.subject.clone());
    }
    for edge in &bundle.edges {
        subjects.insert(edge.from.id.clone(), edge.from.clone());
        subjects.insert(edge.to.id.clone(), edge.to.clone());
    }
    for chain in &bundle.subject_chains {
        for subject in &chain.nodes {
            subjects.insert(subject.id.clone(), subject.clone());
        }
    }
    bundle.subjects = subjects.into_values().collect();
}

pub(super) fn merged_slices(input_slices: Vec<Slice>, bundle: &Bundle) -> Vec<Slice> {
    let mut slices = BTreeMap::new();
    for slice in input_slices {
        slices.insert(slice.id.clone(), slice);
    }
    for slice in slice_by_kind(bundle) {
        slices.insert(slice.id.clone(), slice);
    }
    slices.into_values().collect()
}

pub(super) fn dedupe_by_key<T>(items: &mut Vec<T>, key: impl Fn(&T) -> String) {
    let mut unique = BTreeMap::new();
    for item in items.drain(..) {
        unique.entry(key(&item)).or_insert(item);
    }
    *items = unique.into_values().collect();
}

pub(super) fn producer_key(producer: &reir::subject::Producer) -> String {
    serde_json::to_string(producer).expect("producer should serialize")
}
