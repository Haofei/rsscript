// Public conversion entry points and the sole normal bundle-construction boundary.

pub fn rsscript_to_bundle(
    review_map: &RsScriptReviewMapInput,
    package_review: &RsScriptPackageReviewInput,
) -> Bundle {
    build_rsscript_bundle(
        rsscript_provenance("rsscript-language", PRODUCER_SOURCE),
        review_map_to_facts(review_map)
            .into_iter()
            .chain(package_review_to_facts(package_review)),
        native_boundaries_to_edges(package_review),
    )
    .unwrap_or_else(|error| rsscript_budget_exceeded_bundle(error, PRODUCER_SOURCE))
}

/// Build a REIR bundle from RSScript review-map JSON and package-review JSON.
pub fn rsscript_json_to_bundle(
    review_map_json: Option<&str>,
    package_review_json: Option<&str>,
    package_name: Option<&str>,
) -> Result<Bundle, serde_json::Error> {
    let package_review = match package_review_json {
        Some(json) => Some(package_review_input_from_json(json)?),
        None => None,
    };
    let fallback_package = package_review
        .as_ref()
        .map(|review| review.package_name.as_str())
        .or(package_name)
        .unwrap_or("rsscript");
    let review_map = match review_map_json {
        Some(json) => Some(review_map_input_from_json(fallback_package, json)?),
        None => embedded_review_map_value(package_review_json)?
            .map(|value| review_map_input_from_value(fallback_package, value)),
    };

    let mut facts = Vec::new();
    let mut edges = Vec::new();
    if let Some(review_map) = &review_map {
        facts.extend(review_map_to_facts(review_map));
    }
    if let Some(package_review) = &package_review {
        facts.extend(package_review_to_facts(package_review));
        edges.extend(native_boundaries_to_edges(package_review));
    }
    build_rsscript_bundle(
        rsscript_provenance("rsscript-language", PRODUCER_SOURCE),
        facts,
        edges,
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package lock JSON.
pub fn rsscript_lock_json_to_bundle(lock_json: &str) -> Result<Bundle, serde_json::Error> {
    let lock = package_lock_input_from_json(lock_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-lockfile", LOCKFILE_SOURCE),
        package_lock_to_facts(&lock),
        [],
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package check JSON.
pub fn rsscript_check_json_to_bundle(check_json: &str) -> Result<Bundle, serde_json::Error> {
    let check = package_check_input_from_json(check_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-package-check", PACKAGE_CHECK_SOURCE),
        package_check_to_facts(&check),
        [],
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package lock diff JSON.
pub fn rsscript_lock_diff_json_to_bundle(
    lock_diff_json: &str,
) -> Result<Bundle, serde_json::Error> {
    let diff = package_lock_diff_input_from_json(lock_diff_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-lock-diff", LOCKFILE_SOURCE),
        package_lock_diff_to_facts(&diff),
        [],
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package tree JSON.
pub fn rsscript_tree_json_to_bundle(tree_json: &str) -> Result<Bundle, serde_json::Error> {
    let tree = package_tree_input_from_json(tree_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-package-tree", TREE_SOURCE),
        package_tree_to_facts(&tree),
        package_tree_to_edges(&tree),
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package publish JSON.
pub fn rsscript_publish_json_to_bundle(publish_json: &str) -> Result<Bundle, serde_json::Error> {
    let publish = package_publish_input_from_json(publish_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-publish", PUBLISH_SOURCE),
        package_publish_to_facts(&publish),
        [],
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package metadata JSON.
pub fn rsscript_metadata_json_to_bundle(metadata_json: &str) -> Result<Bundle, serde_json::Error> {
    let metadata = package_metadata_input_from_json(metadata_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-metadata", PACKAGE_METADATA_SOURCE),
        package_metadata_report_to_facts(&metadata),
        [],
    )
    .map_err(adapter_error_to_json)
}

/// Build a REIR bundle from RSScript package vendor JSON.
pub fn rsscript_vendor_json_to_bundle(vendor_json: &str) -> Result<Bundle, serde_json::Error> {
    let vendor = package_vendor_input_from_json(vendor_json)?;
    build_rsscript_bundle(
        rsscript_provenance("rsscript-vendor", VENDOR_SOURCE),
        package_vendor_to_facts(&vendor),
        [],
    )
    .map_err(adapter_error_to_json)
}

fn build_rsscript_bundle(
    producer: ProducerProvenance,
    facts: impl IntoIterator<Item = Fact>,
    edges: impl IntoIterator<Item = Edge>,
) -> Result<Bundle, AdapterBuildError> {
    let mut builder = BoundedEvidenceBuilder::new(RSSCRIPT_ADAPTER_LIMITS);
    builder.extend_facts(facts)?;
    builder.extend_edges(edges)?;
    builder.finish(producer)
}

fn adapter_error_to_json(error: AdapterBuildError) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}
