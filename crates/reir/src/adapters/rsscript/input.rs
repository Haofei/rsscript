// Serialized neutral compiler input accepted by the adapter.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptPackageAnalysisInput {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub producer: Value,
    pub language_version: String,
    pub interface_catalog_digest: String,
    pub snapshot_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_digest: Option<String>,
    pub package: RsScriptPackageIdentityInput,
    pub files: Vec<Value>,
    pub summary: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<RsScriptPackageAnalysisExport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_imports: Vec<RsScriptPackageAnalysisExternalImport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub await_sites: Vec<RsScriptPackageAnalysisAwaitSite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<RsScriptPackageAnalysisDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptPackageIdentityInput {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptPackageAnalysisExport {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_params: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_facts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptPackageAnalysisExternalImport {
    pub function: String,
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<RsScriptDiagnosticSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptPackageAnalysisAwaitSite {
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_across_await: Vec<String>,
    pub span: RsScriptDiagnosticSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptPackageAnalysisDiagnostic {
    pub code: String,
    pub severity: String,
    pub summary: String,
    pub span: RsScriptDiagnosticSpan,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub causes: Vec<String>,
    #[serde(default)]
    pub fixes: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RsScriptDiagnosticSpan {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub column: usize,
    #[serde(default)]
    pub length: usize,
}
