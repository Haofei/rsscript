use super::CliError;
use super::safe_io::{MAX_CLI_INPUT_BYTES, read_bounded_text_accounted};
use reir::adapters::rsscript::rsscript_analysis_json_to_bundle;
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
    pub(super) package_analysis_json: &'a str,
}

pub(super) fn collect_rsscript_bundle(
    inputs: RsscriptCollectInputs<'_>,
) -> Result<Bundle, CliError> {
    rsscript_analysis_json_to_bundle(inputs.package_analysis_json).map_err(|error| {
        CliError::runtime(format!(
            "failed to collect RSScript package analysis: {error}"
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
