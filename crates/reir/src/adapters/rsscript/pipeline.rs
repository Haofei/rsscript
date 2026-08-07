// The sole normal bundle-construction boundary.

/// Build a REIR bundle from neutral `rsscript.package_analysis.v1` JSON.
pub fn rsscript_analysis_json_to_bundle(
    package_analysis_json: &str,
) -> Result<Bundle, serde_json::Error> {
    let analysis = package_analysis_input_from_json(package_analysis_json)?;
    build_rsscript_bundle(package_analysis_to_facts(&analysis)).map_err(adapter_error_to_json)
}

fn build_rsscript_bundle(
    facts: impl IntoIterator<Item = Fact>,
) -> Result<Bundle, AdapterBuildError> {
    let mut builder = BoundedEvidenceBuilder::new(RSSCRIPT_ADAPTER_LIMITS);
    builder.extend_facts(facts)?;
    builder.finish(rsscript_provenance())
}

fn adapter_error_to_json(error: AdapterBuildError) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}
