// Strict JSON decoding into the neutral adapter input.

pub fn package_analysis_input_from_json(
    json: &str,
) -> Result<RsScriptPackageAnalysisInput, serde_json::Error> {
    use serde::de::Error as _;

    let input: RsScriptPackageAnalysisInput = serde_json::from_str(json)?;
    if input.schema != PACKAGE_ANALYSIS_SCHEMA {
        return Err(serde_json::Error::custom(format!(
            "unsupported RSScript package analysis schema `{}`; expected `{PACKAGE_ANALYSIS_SCHEMA}`",
            input.schema
        )));
    }
    Ok(input)
}
