#![forbid(unsafe_code)]

//! Versioned, provider-neutral Artifact Bundle envelopes.
//!
//! This crate owns the persisted Bundle contract. SDK, CLI, runner, review and
//! inspection tools consume it without depending on one another.

use std::error::Error;
use std::fmt;

use rsscript_abi_model::ExternalImport;
use rsscript_bytecode::BytecodeArtifact;
use rsscript_diagnostics::{Diagnostic, Span};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod semantic_diff;

pub use semantic_diff::{
    ArtifactIdentityV1, AwaitFactV1, CallEdgeFactV1, ChangedFactV1, CountChangeV1,
    DiagnosticFactV1, ExportFactV1, ExternalCallFactV1, ExternalContractFactV1, FactSetDiffV1,
    FunctionParameterFactV1, ResourceLifetimeFactV1, ResourceTransferFactV1, SemanticDiffV1,
    TaskGroupFactV1, SEMANTIC_DIFF_SCHEMA,
};

pub const ARTIFACT_BUNDLE_SCHEMA: &str = "rsscript.artifact_bundle.v1";
pub const ARTIFACT_BUNDLE_MAGIC: &[u8; 8] = b"RSSBND\0\x01";
pub const SOURCE_ANALYSIS_SCHEMA: &str = "rsscript.source_analysis.v1";
pub const PACKAGE_ANALYSIS_SCHEMA: &str = "rsscript.package_analysis.v1";

/// Provider- and review-neutral identity of the immutable package that
/// produced an Artifact Bundle or package-analysis evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIdentityV1 {
    pub name: String,
    pub version: String,
    pub edition: String,
}

/// Logical source role recorded in package analysis. This is source evidence,
/// not a review classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageFileKindV1 {
    Interface,
    Source,
    Test,
}

/// Identifies the toolchain rules that emitted package-analysis evidence.
///
/// The producer is part of the persisted analysis contract rather than a
/// compiler-private implementation detail. Consumers can therefore compare
/// evidence produced by different compiler integrations without depending on
/// the compiler crate that happened to emit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisProducerV1 {
    pub name: String,
    pub version: String,
    pub source_revision: String,
    pub ruleset_digest: String,
}

impl PackageAnalysisProducerV1 {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        source_revision: impl Into<String>,
        ruleset_digest: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            source_revision: source_revision.into(),
            ruleset_digest: ruleset_digest.into(),
        }
    }
}

/// Provider- and review-neutral semantic facts for one immutable package
/// snapshot. Host selection, risk classification, native implementation
/// details, and deployment evidence deliberately live outside this artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisV1 {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub producer: PackageAnalysisProducerV1,
    pub language_version: String,
    pub interface_catalog_digest: String,
    /// Digest of the immutable source/interface snapshot analyzed here.
    pub snapshot_digest: String,
    /// Executable payload digest when analysis was emitted by a build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_digest: Option<String>,
    pub package: PackageIdentityV1,
    pub files: Vec<PackageAnalysisFileV1>,
    pub summary: PackageAnalysisSummaryV1,
    pub exports: Vec<PackageAnalysisExportV1>,
    pub external_imports: Vec<PackageAnalysisExternalImportV1>,
    pub call_edges: Vec<PackageAnalysisCallEdgeV1>,
    pub recursive_functions: Vec<String>,
    pub resource_lifetimes: Vec<PackageAnalysisResourceLifetimeV1>,
    pub resource_transfers: Vec<PackageAnalysisResourceTransferV1>,
    pub task_groups: Vec<PackageAnalysisTaskGroupV1>,
    pub await_sites: Vec<PackageAnalysisAwaitSiteV1>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisSummaryV1 {
    pub interface_files: usize,
    pub source_files: usize,
    pub public_types: usize,
    pub public_sum_types: usize,
    pub public_type_aliases: usize,
    pub public_consts: usize,
    pub public_functions: usize,
    pub mutating_apis: usize,
    pub retaining_apis: usize,
    pub resource_apis: usize,
    pub fresh_returning_apis: usize,
    pub async_apis: usize,
    pub await_sites: usize,
    pub diagnostics: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisExportV1 {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<PackageAnalysisParameterV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retained_params: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub semantic_facts: Vec<String>,
}

/// Source-level public function parameter contract captured in neutral package
/// analysis. `effect` is explicit even for ordinary `read` parameters so a
/// semantic diff never has to infer ownership behavior from omitted syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisParameterV1 {
    pub name: String,
    pub effect: String,
    pub ty: String,
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisExternalImportV1 {
    pub function: String,
    pub symbol: String,
    pub call_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

/// One resolved call edge in the package-owned call graph. This is neutral
/// semantic evidence, not a review classification or deployment decision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisCallEdgeV1 {
    pub caller: String,
    pub callee: String,
}

/// A lexical `with` resource lifetime. Scope exit cleanup is language
/// semantics, so normal completion, error unwinding and cancellation share the
/// same cleanup fact without exposing a deployment policy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisResourceLifetimeV1 {
    pub function: String,
    pub binding: String,
    pub acquisition: String,
    pub cleanup: String,
    pub cleanup_on_cancellation: bool,
}

/// An explicit ownership transfer of a lexically managed resource. Only a
/// `take` applied to a binding introduced by `with` is recorded, so ordinary
/// value moves cannot be mistaken for a resource hand-off.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisResourceTransferV1 {
    pub function: String,
    pub binding: String,
    pub operation: String,
}

/// Structured concurrency owned by one lexical task group.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisTaskGroupV1 {
    pub function: String,
    pub spawned_tasks: u32,
    pub select_arms: u32,
    pub drains_on_exit: bool,
    pub cleanup_on_cancellation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisAwaitSiteV1 {
    pub function: String,
    pub callee: Option<String>,
    pub live_across_await: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAnalysisFileV1 {
    pub path: String,
    pub kind: PackageFileKindV1,
}
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ANALYSIS_BYTES: usize = 16 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

/// The versioned analysis families that a Bundle v1 may carry.
///
/// The payload remains JSON during the v1 compatibility window, but consumers
/// must select a known schema through this enum rather than interpreting an
/// unbounded `$schema` string themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisSchemaV1 {
    Source,
    Package,
}

impl AnalysisSchemaV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => SOURCE_ANALYSIS_SCHEMA,
            Self::Package => PACKAGE_ANALYSIS_SCHEMA,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            SOURCE_ANALYSIS_SCHEMA => Some(Self::Source),
            PACKAGE_ANALYSIS_SCHEMA => Some(Self::Package),
            _ => None,
        }
    }
}

/// A validated analysis section from one Artifact Bundle.
///
/// This is the stable section boundary. Individual source/package analysis
/// payloads can evolve behind their own schemas without turning Bundle loading
/// into ad-hoc `$schema` string inspection at every consumer.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisEnvelopeV1 {
    schema: AnalysisSchemaV1,
    payload: Value,
    source: Option<SourceAnalysisV1>,
    package: Option<PackageAnalysisV1>,
}

impl AnalysisEnvelopeV1 {
    /// Construct the typed analysis evidence emitted by an in-memory source
    /// compilation.
    ///
    /// `source_analysis.v1` is intentionally not accepted as an arbitrary JSON
    /// object at the producer boundary. This keeps the compiler, SDK, and
    /// bundle writer aligned on one versioned evidence shape.
    pub fn source(source: SourceAnalysisV1) -> Self {
        let payload = serde_json::json!({
            "$schema": SOURCE_ANALYSIS_SCHEMA,
            "language_version": source.language_version.clone(),
            "snapshot_digest": source.snapshot_digest.clone(),
            "sources": source.sources.clone(),
            "exports": source.exports.clone(),
            "call_edges": source.call_edges.clone(),
            "external_calls": source.external_calls.clone(),
        });
        Self {
            schema: AnalysisSchemaV1::Source,
            payload,
            source: Some(source),
            package: None,
        }
    }

    /// Construct the typed, provider-neutral evidence emitted by an immutable
    /// package snapshot.
    pub fn package(package: PackageAnalysisV1) -> Result<Self, ArtifactBundleError> {
        if package.schema != PACKAGE_ANALYSIS_SCHEMA {
            return Err(ArtifactBundleError::UnsupportedAnalysisSchema(
                package.schema,
            ));
        }
        let payload = serde_json::to_value(&package)
            .map_err(|error| ArtifactBundleError::Analysis(error.to_string()))?;
        Ok(Self {
            schema: AnalysisSchemaV1::Package,
            payload,
            source: None,
            package: Some(package),
        })
    }

    /// Decode analysis evidence read from a persisted Bundle or produced by a
    /// legacy package-analysis adapter.
    ///
    /// Both supported analysis schemas are decoded through their versioned
    /// models so malformed or silently extended evidence cannot enter a
    /// verified bundle.
    pub fn from_json(payload: Value) -> Result<Self, ArtifactBundleError> {
        let schema = payload
            .get("$schema")
            .and_then(Value::as_str)
            .ok_or(ArtifactBundleError::MissingAnalysisSchema)?;
        let schema = AnalysisSchemaV1::parse(schema)
            .ok_or_else(|| ArtifactBundleError::UnsupportedAnalysisSchema(schema.to_string()))?;
        if schema == AnalysisSchemaV1::Source {
            // Preserve the exact accepted v1 source-analysis shape. Early v1
            // bundles omitted empty optional fact arrays; rebuilding through
            // `Self::source` would insert them and make a historically compact
            // (otherwise valid) analysis section fail its byte-level legacy
            // canonicality check. New producers still use `Self::source` and
            // therefore emit the complete canonical form.
            let source = SourceAnalysisV1::from_json(payload.clone())?;
            return Ok(Self {
                schema,
                payload,
                source: Some(source),
                package: None,
            });
        }
        let package = serde_json::from_value(payload)
            .map_err(|error| ArtifactBundleError::Analysis(error.to_string()))?;
        Self::package(package)
    }

    pub const fn schema(&self) -> AnalysisSchemaV1 {
        self.schema
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }

    pub fn into_payload(self) -> Value {
        self.payload
    }

    /// Return typed direct-source evidence when this envelope carries the
    /// `source_analysis.v1` schema. Package evidence remains a separate
    /// migration schema and intentionally does not pretend to be this type.
    pub fn source_analysis(&self) -> Option<&SourceAnalysisV1> {
        self.source.as_ref()
    }

    /// Typed package evidence when this envelope carries
    /// `rsscript.package_analysis.v1`.
    pub fn package_analysis(&self) -> Option<&PackageAnalysisV1> {
        self.package.as_ref()
    }
}

/// The complete, versioned evidence emitted by a direct source build.
///
/// This type deliberately excludes the `$schema` discriminator: the
/// [`AnalysisEnvelopeV1`] owns that wire-level choice, so producers cannot
/// accidentally label a source analysis with a different schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAnalysisV1 {
    pub language_version: String,
    pub snapshot_digest: String,
    pub sources: Vec<String>,
    /// Checked source function contracts. These use the same neutral fact
    /// model as package analysis, so semantic diffs can compare ownership and
    /// retention changes without reparsing source text or consulting review
    /// policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<ExportFactV1>,
    /// Resolved direct calls from checked semantic input. These are neutral
    /// program facts, not host authorization or review policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_edges: Vec<CallEdgeFactV1>,
    /// Resolved external calls with their direct caller. A full package
    /// analysis may add transitive call chains; direct source snapshots retain
    /// the checked call-site fact available at their own boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_calls: Vec<ExternalCallFactV1>,
}

impl SourceAnalysisV1 {
    pub fn new(
        language_version: impl Into<String>,
        snapshot_digest: impl Into<String>,
        sources: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut sources = sources.into_iter().map(Into::into).collect::<Vec<_>>();
        sources.sort_unstable();
        Self {
            language_version: language_version.into(),
            snapshot_digest: snapshot_digest.into(),
            sources,
            exports: Vec::new(),
            call_edges: Vec::new(),
            external_calls: Vec::new(),
        }
    }

    /// Attach canonically ordered checked function contracts derived from the
    /// same validated input that produced this Artifact.
    pub fn with_function_contracts(mut self, mut exports: Vec<ExportFactV1>) -> Self {
        exports.sort();
        exports.dedup();
        self.exports = exports;
        self
    }

    /// Attach canonically ordered semantic call facts derived from the same
    /// validated input that produced this Artifact.
    pub fn with_call_facts(
        mut self,
        mut call_edges: Vec<CallEdgeFactV1>,
        mut external_calls: Vec<ExternalCallFactV1>,
    ) -> Self {
        call_edges.sort();
        call_edges.dedup();
        external_calls.sort();
        external_calls.dedup();
        self.call_edges = call_edges;
        self.external_calls = external_calls;
        self
    }

    fn from_json(payload: Value) -> Result<Self, ArtifactBundleError> {
        let mut object = payload.as_object().cloned().ok_or_else(|| {
            ArtifactBundleError::Analysis("source analysis must be a JSON object".to_string())
        })?;
        let schema = object
            .remove("$schema")
            .and_then(|value| value.as_str().map(ToOwned::to_owned));
        if schema.as_deref() != Some(SOURCE_ANALYSIS_SCHEMA) {
            return Err(ArtifactBundleError::UnsupportedAnalysisSchema(
                schema.unwrap_or_default(),
            ));
        }
        serde_json::from_value(Value::Object(object))
            .map_err(|error| ArtifactBundleError::Analysis(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildProvenanceV1 {
    pub compiler_version: String,
    pub language_version: String,
    pub core_library_abi_version: u32,
    pub runtime_abi_version: u32,
    pub interface_catalog_digest: String,
    pub source_content_hash: String,
    pub snapshot_digest: String,
    pub module_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceRequirementV1 {
    pub symbol: String,
    pub signature_hash: String,
    pub abi_version: u32,
}

impl From<&ExternalImport> for InterfaceRequirementV1 {
    fn from(import: &ExternalImport) -> Self {
        Self {
            symbol: import.symbol.as_str().to_string(),
            signature_hash: import.signature_hash.as_str().to_string(),
            abi_version: import.abi_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifestV1 {
    schema: String,
    provenance: BuildProvenanceV1,
    required_interfaces: Vec<InterfaceRequirementV1>,
    artifact_digest: String,
    analysis_digest: String,
}

/// A provider-neutral deployable unit of executable bytes, semantic evidence,
/// provenance, and exact external interface requirements.
#[derive(Debug, Clone)]
pub struct ArtifactBundle {
    manifest: BundleManifestV1,
    artifact: Vec<u8>,
    analysis: AnalysisEnvelopeV1,
    external_contracts: Vec<ExternalImport>,
    digest: String,
}

impl ArtifactBundle {
    /// Construct a Bundle from independently produced executable bytes and
    /// versioned analysis evidence.
    pub fn new(
        artifact: Vec<u8>,
        analysis: AnalysisEnvelopeV1,
    ) -> Result<Self, ArtifactBundleError> {
        let envelope = BytecodeArtifact::from_bytes(&artifact)
            .map_err(|error| ArtifactBundleError::Artifact(error.to_string()))?;
        let analysis_bytes = canonical_analysis_json(&analysis)?;
        let manifest = BundleManifestV1 {
            schema: ARTIFACT_BUNDLE_SCHEMA.to_string(),
            provenance: BuildProvenanceV1 {
                compiler_version: envelope.header.compiler_version.clone(),
                language_version: envelope.header.language_version.clone(),
                core_library_abi_version: envelope.header.core_library_abi_version,
                runtime_abi_version: envelope.header.runtime_abi_version,
                interface_catalog_digest: envelope.header.interface_catalog_digest.clone(),
                source_content_hash: envelope.header.source_content_hash.clone(),
                snapshot_digest: envelope
                    .header
                    .snapshot_digest
                    .clone()
                    .ok_or(ArtifactBundleError::MissingSnapshotDigest)?,
                module_digest: envelope.header.executable_hash.clone(),
            },
            required_interfaces: envelope
                .imports
                .iter()
                .map(InterfaceRequirementV1::from)
                .collect(),
            artifact_digest: digest(&artifact),
            analysis_digest: digest(&analysis_bytes),
        };
        let manifest_bytes = canonical_json(&manifest)?;
        Self::from_sections(manifest, manifest_bytes, artifact, analysis, analysis_bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ArtifactBundleError> {
        let mut input = bytes
            .strip_prefix(ARTIFACT_BUNDLE_MAGIC)
            .ok_or(ArtifactBundleError::InvalidMagic)?;
        let manifest_len = take_length(&mut input, MAX_MANIFEST_BYTES)?;
        let artifact_len = take_length(&mut input, MAX_ARTIFACT_BYTES)?;
        let analysis_len = take_length(&mut input, MAX_ANALYSIS_BYTES)?;
        let manifest_bytes = take(&mut input, manifest_len)?.to_vec();
        let manifest: BundleManifestV1 = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| ArtifactBundleError::Manifest(error.to_string()))?;
        validate_v1_manifest_encoding(&manifest, &manifest_bytes)?;
        let artifact = take(&mut input, artifact_len)?.to_vec();
        let analysis_bytes = take(&mut input, analysis_len)?.to_vec();
        if !input.is_empty() {
            return Err(ArtifactBundleError::TrailingBytes);
        }
        let analysis: Value = serde_json::from_slice(&analysis_bytes)
            .map_err(|error| ArtifactBundleError::Analysis(error.to_string()))?;
        let analysis = AnalysisEnvelopeV1::from_json(analysis)?;
        validate_v1_analysis_encoding(&analysis, &analysis_bytes)?;
        Self::from_sections(manifest, manifest_bytes, artifact, analysis, analysis_bytes)
    }

    fn from_sections(
        manifest: BundleManifestV1,
        manifest_bytes: Vec<u8>,
        artifact: Vec<u8>,
        analysis: AnalysisEnvelopeV1,
        analysis_bytes: Vec<u8>,
    ) -> Result<Self, ArtifactBundleError> {
        if manifest.schema != ARTIFACT_BUNDLE_SCHEMA {
            return Err(ArtifactBundleError::UnsupportedSchema(manifest.schema));
        }
        if manifest.artifact_digest != digest(&artifact) {
            return Err(ArtifactBundleError::ArtifactDigestMismatch);
        }
        if manifest.analysis_digest != digest(&analysis_bytes) {
            return Err(ArtifactBundleError::AnalysisDigestMismatch);
        }
        let envelope = BytecodeArtifact::from_bytes(&artifact)
            .map_err(|error| ArtifactBundleError::Artifact(error.to_string()))?;
        let expected = envelope
            .imports
            .iter()
            .map(InterfaceRequirementV1::from)
            .collect::<Vec<_>>();
        if manifest.provenance.module_digest != envelope.header.executable_hash
            || manifest.provenance.compiler_version != envelope.header.compiler_version
            || manifest.provenance.language_version != envelope.header.language_version
            || manifest.provenance.core_library_abi_version
                != envelope.header.core_library_abi_version
            || manifest.provenance.runtime_abi_version != envelope.header.runtime_abi_version
            || manifest.provenance.interface_catalog_digest
                != envelope.header.interface_catalog_digest
            || manifest.provenance.source_content_hash != envelope.header.source_content_hash
            || envelope.header.snapshot_digest.as_deref()
                != Some(manifest.provenance.snapshot_digest.as_str())
            || manifest.required_interfaces != expected
        {
            return Err(ArtifactBundleError::ManifestArtifactMismatch);
        }
        let digest = bundle_digest(&manifest_bytes, &artifact, &analysis_bytes);
        Ok(Self {
            manifest,
            artifact,
            analysis,
            external_contracts: envelope.imports,
            digest,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ArtifactBundleError> {
        let analysis = canonical_analysis_json(&self.analysis)?;
        // A reader may have accepted a historical compact JSON section. Every
        // writer normalizes it back to canonical bytes and must consequently
        // refresh the manifest's section digests before serializing.
        let mut normalized_manifest = self.manifest.clone();
        normalized_manifest.artifact_digest = digest(&self.artifact);
        normalized_manifest.analysis_digest = digest(&analysis);
        let manifest = canonical_json(&normalized_manifest)?;
        let mut output = Vec::with_capacity(
            ARTIFACT_BUNDLE_MAGIC.len()
                + 24
                + manifest.len()
                + self.artifact.len()
                + analysis.len(),
        );
        output.extend_from_slice(ARTIFACT_BUNDLE_MAGIC);
        for length in [manifest.len(), self.artifact.len(), analysis.len()] {
            put_length(&mut output, length)?;
        }
        output.extend_from_slice(&manifest);
        output.extend_from_slice(&self.artifact);
        output.extend_from_slice(&analysis);
        Ok(output)
    }

    pub fn artifact_bytes(&self) -> &[u8] {
        &self.artifact
    }
    pub fn analysis(&self) -> &serde_json::Value {
        self.analysis.payload()
    }
    pub fn analysis_envelope(&self) -> &AnalysisEnvelopeV1 {
        &self.analysis
    }
    /// Typed direct-source evidence, if this Bundle was built from an
    /// in-memory source/interface snapshot.
    pub fn source_analysis(&self) -> Option<&SourceAnalysisV1> {
        self.analysis.source_analysis()
    }
    /// Typed package evidence, if this Bundle was built from an immutable
    /// package snapshot.
    pub fn package_analysis(&self) -> Option<&PackageAnalysisV1> {
        self.analysis.package_analysis()
    }
    pub fn provenance(&self) -> &BuildProvenanceV1 {
        &self.manifest.provenance
    }
    pub fn required_interfaces(&self) -> &[InterfaceRequirementV1] {
        &self.manifest.required_interfaces
    }
    /// Full structured import contracts for inspection and semantic diffing.
    pub fn external_contracts(&self) -> &[ExternalImport] {
        &self.external_contracts
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Encode JSON sections using the Bundle v1 canonical form.
///
/// Object keys are serialized in Unicode scalar-value order at every depth.
/// This deliberately does not rely on `serde_json::Map`'s backing type: the
/// workspace may enable `preserve_order` for other consumers, but an Artifact
/// digest must not change when an equivalent JSON object is constructed with a
/// different insertion order.
fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, ArtifactBundleError> {
    let value = serde_json::to_value(value)
        .map_err(|error| ArtifactBundleError::Manifest(error.to_string()))?;
    let mut output = Vec::new();
    write_canonical_json(&mut output, &value)?;
    Ok(output)
}

/// Bundle v1 originally used `serde_json::to_vec` directly. That encoding is
/// compact and deterministic for one producer, but its object ordering is not
/// a portable canonical contract. Readers accept that historical form so
/// already-produced v1 Bundles remain deployable; writers always use
/// [`canonical_json`] and therefore never create another legacy representation.
///
/// This is deliberately narrower than accepting arbitrary JSON whitespace:
/// decoded values must re-encode byte-for-byte with the historical compact
/// serializer. Section hashes and the manifest's section digests still bind the
/// exact accepted bytes.
fn validate_v1_manifest_encoding(
    value: &impl Serialize,
    bytes: &[u8],
) -> Result<(), ArtifactBundleError> {
    if canonical_json(value)? == bytes || legacy_compact_json(value)? == bytes {
        return Ok(());
    }
    Err(ArtifactBundleError::NonCanonicalManifest)
}

/// Validate the analysis section without turning every compact JSON object
/// into a historical compatibility encoding. Source analysis used to be
/// serialized from a typed v1 record; accepting `serde_json::to_vec(Value)`
/// here would instead accept any key order preserved by the parser. That would
/// silently weaken the canonical-encoding boundary.
fn validate_v1_analysis_encoding(
    analysis: &AnalysisEnvelopeV1,
    bytes: &[u8],
) -> Result<(), ArtifactBundleError> {
    if canonical_json(analysis.payload())? == bytes
        || legacy_compact_analysis_json(analysis)? == bytes
    {
        return Ok(());
    }
    Err(ArtifactBundleError::NonCanonicalAnalysis)
}

fn legacy_compact_analysis_json(
    analysis: &AnalysisEnvelopeV1,
) -> Result<Vec<u8>, ArtifactBundleError> {
    match (analysis.source_analysis(), analysis.package_analysis()) {
        (Some(source), None) => {
            #[derive(Serialize)]
            struct LegacySourceAnalysis<'a> {
                #[serde(rename = "$schema")]
                schema: &'static str,
                language_version: &'a str,
                snapshot_digest: &'a str,
                sources: &'a [String],
                #[serde(skip_serializing_if = "slice_is_empty")]
                exports: &'a [ExportFactV1],
                #[serde(skip_serializing_if = "slice_is_empty")]
                call_edges: &'a [CallEdgeFactV1],
                #[serde(skip_serializing_if = "slice_is_empty")]
                external_calls: &'a [ExternalCallFactV1],
            }

            legacy_compact_json(&LegacySourceAnalysis {
                schema: SOURCE_ANALYSIS_SCHEMA,
                language_version: &source.language_version,
                snapshot_digest: &source.snapshot_digest,
                sources: &source.sources,
                exports: &source.exports,
                call_edges: &source.call_edges,
                external_calls: &source.external_calls,
            })
        }
        (None, Some(package)) => legacy_compact_json(package),
        _ => Err(ArtifactBundleError::Analysis(
            "analysis envelope has an invalid schema/payload pairing".to_string(),
        )),
    }
}

/// Writers normalize source evidence through its typed model. This fills the
/// optional empty fact arrays omitted by early v1 producers, so reading a
/// historical compact Bundle and writing it again always produces the current
/// canonical representation.
fn canonical_analysis_json(analysis: &AnalysisEnvelopeV1) -> Result<Vec<u8>, ArtifactBundleError> {
    match (analysis.source_analysis(), analysis.package_analysis()) {
        (Some(source), None) => {
            let normalized = AnalysisEnvelopeV1::source(source.clone());
            canonical_json(normalized.payload())
        }
        (None, Some(package)) => canonical_json(package),
        _ => Err(ArtifactBundleError::Analysis(
            "analysis envelope has an invalid schema/payload pairing".to_string(),
        )),
    }
}

fn slice_is_empty<T>(values: &[T]) -> bool {
    values.is_empty()
}

fn legacy_compact_json(value: &impl Serialize) -> Result<Vec<u8>, ArtifactBundleError> {
    serde_json::to_vec(value).map_err(|error| ArtifactBundleError::Manifest(error.to_string()))
}

fn write_canonical_json(output: &mut Vec<u8>, value: &Value) -> Result<(), ArtifactBundleError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => {
            if *value {
                output.extend_from_slice(b"true");
            } else {
                output.extend_from_slice(b"false");
            }
        }
        Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => output.extend_from_slice(
            serde_json::to_string(value)
                .map_err(|error| ArtifactBundleError::Manifest(error.to_string()))?
                .as_bytes(),
        ),
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(output, value)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(
                    serde_json::to_string(key)
                        .map_err(|error| ArtifactBundleError::Manifest(error.to_string()))?
                        .as_bytes(),
                );
                output.push(b':');
                write_canonical_json(
                    output,
                    values
                        .get(key)
                        .expect("canonical JSON key came from object"),
                )?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn bundle_digest(manifest: &[u8], artifact: &[u8], analysis: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rsscript.artifact_bundle.v1.digest\0");
    for (name, section) in [
        (b"manifest".as_slice(), manifest),
        (b"artifact".as_slice(), artifact),
        (b"analysis".as_slice(), analysis),
    ] {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name);
        hasher.update((section.len() as u64).to_be_bytes());
        hasher.update(section);
    }
    format!("sha256:{:x}", hasher.finalize())
}
fn put_length(output: &mut Vec<u8>, length: usize) -> Result<(), ArtifactBundleError> {
    output.extend_from_slice(
        &u64::try_from(length)
            .map_err(|_| ArtifactBundleError::LengthOverflow)?
            .to_be_bytes(),
    );
    Ok(())
}
fn take_length(input: &mut &[u8], maximum: usize) -> Result<usize, ArtifactBundleError> {
    let bytes: [u8; 8] = take(input, 8)?
        .try_into()
        .map_err(|_| ArtifactBundleError::Truncated)?;
    let length = usize::try_from(u64::from_be_bytes(bytes))
        .map_err(|_| ArtifactBundleError::LengthOverflow)?;
    if length > maximum {
        Err(ArtifactBundleError::SectionTooLarge { length, maximum })
    } else {
        Ok(length)
    }
}
fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8], ArtifactBundleError> {
    if input.len() < length {
        return Err(ArtifactBundleError::Truncated);
    }
    let (head, tail) = input.split_at(length);
    *input = tail;
    Ok(head)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactBundleError {
    InvalidMagic,
    Truncated,
    TrailingBytes,
    LengthOverflow,
    SectionTooLarge { length: usize, maximum: usize },
    UnsupportedSchema(String),
    MissingAnalysisSchema,
    MissingSnapshotDigest,
    UnsupportedAnalysisSchema(String),
    Manifest(String),
    Analysis(String),
    Artifact(String),
    NonCanonicalManifest,
    NonCanonicalAnalysis,
    ArtifactDigestMismatch,
    AnalysisDigestMismatch,
    ManifestArtifactMismatch,
}
impl fmt::Display for ArtifactBundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => f.write_str("invalid RSScript Artifact Bundle magic"),
            Self::Truncated => f.write_str("truncated RSScript Artifact Bundle"),
            Self::TrailingBytes => f.write_str("Artifact Bundle has trailing bytes"),
            Self::LengthOverflow => f.write_str("Artifact Bundle section length overflow"),
            Self::SectionTooLarge { length, maximum } => write!(
                f,
                "Artifact Bundle section is too large ({length} bytes; maximum {maximum})"
            ),
            Self::UnsupportedSchema(schema) => {
                write!(f, "unsupported Artifact Bundle schema `{schema}`")
            }
            Self::MissingAnalysisSchema => f.write_str("bundle analysis has no declared schema"),
            Self::MissingSnapshotDigest => {
                f.write_str("bundle artifact has no immutable snapshot digest")
            }
            Self::UnsupportedAnalysisSchema(schema) => {
                write!(f, "unsupported bundle analysis schema `{schema}`")
            }
            Self::Manifest(message) => write!(f, "invalid bundle manifest: {message}"),
            Self::Analysis(message) => write!(f, "invalid bundle analysis: {message}"),
            Self::Artifact(message) => write!(f, "invalid bundled artifact: {message}"),
            Self::NonCanonicalManifest => f.write_str("bundle manifest is not canonically encoded"),
            Self::NonCanonicalAnalysis => f.write_str("bundle analysis is not canonically encoded"),
            Self::ArtifactDigestMismatch => f.write_str("artifact digest mismatch"),
            Self::AnalysisDigestMismatch => f.write_str("analysis digest mismatch"),
            Self::ManifestArtifactMismatch => {
                f.write_str("bundle manifest does not match the bytecode artifact")
            }
        }
    }
}
impl Error for ArtifactBundleError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> ArtifactBundle {
        let mut artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            "sha256:catalog",
            2,
            "sha256:source",
            vec![],
            vec![1, 2, 3],
        )
        .unwrap();
        artifact
            .bind_snapshot_digest(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap();
        ArtifactBundle::new(
            artifact.to_bytes().unwrap(),
            AnalysisEnvelopeV1::source(SourceAnalysisV1::new(
                "0.1.0",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ["src/main.rss"],
            )),
        )
        .unwrap()
    }

    fn package_bundle() -> ArtifactBundle {
        let mut artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            "sha256:catalog",
            2,
            "sha256:source",
            vec![],
            vec![1, 2, 3],
        )
        .unwrap();
        artifact
            .bind_snapshot_digest(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap();
        let module_digest = artifact.header.executable_hash.clone();
        let analysis = PackageAnalysisV1 {
            schema: PACKAGE_ANALYSIS_SCHEMA.to_string(),
            producer: PackageAnalysisProducerV1::new("rsscript", "0.1.0", "test", "sha256:rules"),
            language_version: "0.1.0".to_string(),
            interface_catalog_digest: "sha256:catalog".to_string(),
            snapshot_digest:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            module_digest: Some(module_digest),
            package: PackageIdentityV1 {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                edition: "2024".to_string(),
            },
            files: vec![PackageAnalysisFileV1 {
                path: "src/main.rss".to_string(),
                kind: PackageFileKindV1::Source,
            }],
            summary: PackageAnalysisSummaryV1 {
                source_files: 1,
                ..PackageAnalysisSummaryV1::default()
            },
            exports: vec![],
            external_imports: vec![],
            call_edges: vec![],
            recursive_functions: vec![],
            resource_lifetimes: vec![],
            resource_transfers: vec![],
            task_groups: vec![],
            await_sites: vec![],
            diagnostics: vec![],
        };
        ArtifactBundle::new(
            artifact.to_bytes().unwrap(),
            AnalysisEnvelopeV1::package(analysis).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn bundle_round_trip_binds_artifact_analysis_and_provenance() {
        let original = bundle();
        let decoded = ArtifactBundle::from_bytes(&original.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded.digest(), original.digest());
        assert_eq!(decoded.artifact_bytes(), original.artifact_bytes());
        assert_eq!(decoded.analysis(), original.analysis());
        assert_eq!(
            decoded.analysis_envelope().schema(),
            AnalysisSchemaV1::Source
        );
        assert_eq!(
            decoded
                .source_analysis()
                .expect("source Bundle exposes typed evidence")
                .sources,
            ["src/main.rss"]
        );
        assert_eq!(
            decoded.provenance().snapshot_digest,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn bundle_digest_is_domain_separated_from_raw_section_concatenation() {
        let bundle = bundle();
        let manifest = canonical_json(&bundle.manifest).unwrap();
        let analysis = canonical_json(bundle.analysis.payload()).unwrap();
        let legacy = {
            let mut hasher = Sha256::new();
            for section in [
                manifest.as_slice(),
                bundle.artifact.as_slice(),
                analysis.as_slice(),
            ] {
                hasher.update((section.len() as u64).to_be_bytes());
                hasher.update(section);
            }
            format!("sha256:{:x}", hasher.finalize())
        };
        assert_ne!(bundle.digest(), legacy);
    }

    #[test]
    fn bundle_rejects_tampered_analysis() {
        let mut bytes = bundle().to_bytes().unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(ArtifactBundle::from_bytes(&bytes).is_err());
    }

    #[test]
    fn source_analysis_rejects_unknown_fields_at_the_artifact_boundary() {
        let error = AnalysisEnvelopeV1::from_json(serde_json::json!({
            "$schema": SOURCE_ANALYSIS_SCHEMA,
            "language_version": "0.1.0",
            "snapshot_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sources": ["src/main.rss"],
            "unreviewed_extension": true,
        }))
        .expect_err("typed source analysis must reject unversioned fields");
        assert!(matches!(error, ArtifactBundleError::Analysis(_)));
    }

    #[test]
    fn source_analysis_preserves_the_complete_compiler_input_listing() {
        let source = SourceAnalysisV1::new(
            "0.1.0",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ["src/main.rss", "src/main.rss"],
        );
        let envelope = AnalysisEnvelopeV1::source(source);
        assert_eq!(
            envelope.payload()["sources"],
            serde_json::json!(["src/main.rss", "src/main.rss"])
        );
    }

    #[test]
    fn package_analysis_round_trips_as_typed_evidence() {
        let original = package_bundle();
        let decoded = ArtifactBundle::from_bytes(&original.to_bytes().unwrap()).unwrap();
        let analysis = decoded
            .package_analysis()
            .expect("package Bundle exposes typed evidence");
        assert_eq!(analysis.package.name, "demo");
        assert_eq!(analysis.files[0].kind, PackageFileKindV1::Source);
        assert_eq!(analysis.summary.source_files, 1);
        assert_eq!(
            decoded.analysis_envelope().schema(),
            AnalysisSchemaV1::Package
        );
    }

    #[test]
    fn package_analysis_rejects_unknown_fields_at_the_artifact_boundary() {
        let original = package_bundle();
        let mut payload = serde_json::to_value(
            original
                .package_analysis()
                .expect("package Bundle has typed evidence"),
        )
        .unwrap();
        payload["unreviewed_extension"] = serde_json::json!(true);
        let error = AnalysisEnvelopeV1::from_json(payload)
            .expect_err("typed package analysis must reject unversioned fields");
        assert!(matches!(error, ArtifactBundleError::Analysis(_)));
    }

    #[test]
    fn canonical_json_sorts_nested_object_keys_independently_of_insertion_order() {
        let left = serde_json::json!({
            "$schema": SOURCE_ANALYSIS_SCHEMA,
            "outer": { "z": 1, "a": 2 },
        });
        let right = serde_json::json!({
            "outer": { "a": 2, "z": 1 },
            "$schema": SOURCE_ANALYSIS_SCHEMA,
        });
        assert_eq!(
            canonical_json(&left).unwrap(),
            canonical_json(&right).unwrap()
        );
    }

    #[test]
    fn bundle_rejects_equivalent_but_noncanonical_analysis_json() {
        let bundle = bundle();
        let manifest = canonical_json(&bundle.manifest).unwrap();
        let noncanonical = format!(
            r#"{{"sources":["src/main.rss"],"snapshot_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","language_version":"0.1.0","$schema":"{}"}}"#,
            SOURCE_ANALYSIS_SCHEMA
        );
        let mut bytes = ARTIFACT_BUNDLE_MAGIC.to_vec();
        put_length(&mut bytes, manifest.len()).unwrap();
        put_length(&mut bytes, bundle.artifact.len()).unwrap();
        put_length(&mut bytes, noncanonical.len()).unwrap();
        bytes.extend_from_slice(&manifest);
        bytes.extend_from_slice(&bundle.artifact);
        bytes.extend_from_slice(noncanonical.as_bytes());

        assert!(matches!(
            ArtifactBundle::from_bytes(&bytes),
            Err(ArtifactBundleError::NonCanonicalAnalysis)
        ));
    }

    #[test]
    fn bundle_rejects_equivalent_but_noncanonical_manifest_json() {
        let bundle = bundle();
        let canonical_manifest = canonical_json(&bundle.manifest).unwrap();
        let mut noncanonical = Vec::with_capacity(canonical_manifest.len() + 1);
        noncanonical.push(b' ');
        noncanonical.extend_from_slice(&canonical_manifest);
        assert_ne!(canonical_json(&bundle.manifest).unwrap(), noncanonical);

        let analysis = canonical_json(bundle.analysis.payload()).unwrap();
        let mut bytes = ARTIFACT_BUNDLE_MAGIC.to_vec();
        put_length(&mut bytes, noncanonical.len()).unwrap();
        put_length(&mut bytes, bundle.artifact.len()).unwrap();
        put_length(&mut bytes, analysis.len()).unwrap();
        bytes.extend_from_slice(&noncanonical);
        bytes.extend_from_slice(&bundle.artifact);
        bytes.extend_from_slice(&analysis);

        assert!(matches!(
            ArtifactBundle::from_bytes(&bytes),
            Err(ArtifactBundleError::NonCanonicalManifest)
        ));
    }

    #[test]
    fn reader_accepts_the_historical_compact_v1_manifest_and_normalizes_on_write() {
        let bundle = bundle();
        let legacy_manifest = legacy_compact_json(&bundle.manifest).unwrap();
        let canonical_manifest = canonical_json(&bundle.manifest).unwrap();
        assert_ne!(legacy_manifest, canonical_manifest);
        let analysis = canonical_json(bundle.analysis.payload()).unwrap();

        let mut bytes = ARTIFACT_BUNDLE_MAGIC.to_vec();
        put_length(&mut bytes, legacy_manifest.len()).unwrap();
        put_length(&mut bytes, bundle.artifact.len()).unwrap();
        put_length(&mut bytes, analysis.len()).unwrap();
        bytes.extend_from_slice(&legacy_manifest);
        bytes.extend_from_slice(&bundle.artifact);
        bytes.extend_from_slice(&analysis);

        let decoded = ArtifactBundle::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.to_bytes().unwrap(), bundle.to_bytes().unwrap());
    }

    #[test]
    fn reader_accepts_only_typed_historical_analysis_and_normalizes_on_write() {
        let bundle = bundle();
        let legacy_analysis = legacy_compact_analysis_json(&bundle.analysis).unwrap();
        let canonical_analysis = canonical_json(bundle.analysis.payload()).unwrap();
        assert_ne!(legacy_analysis, canonical_analysis);

        let mut manifest = bundle.manifest.clone();
        manifest.analysis_digest = digest(&legacy_analysis);
        let manifest = canonical_json(&manifest).unwrap();
        let mut bytes = ARTIFACT_BUNDLE_MAGIC.to_vec();
        put_length(&mut bytes, manifest.len()).unwrap();
        put_length(&mut bytes, bundle.artifact.len()).unwrap();
        put_length(&mut bytes, legacy_analysis.len()).unwrap();
        bytes.extend_from_slice(&manifest);
        bytes.extend_from_slice(&bundle.artifact);
        bytes.extend_from_slice(&legacy_analysis);

        let decoded = ArtifactBundle::from_bytes(&bytes).expect("typed v1 analysis is accepted");
        assert_eq!(decoded.to_bytes().unwrap(), bundle.to_bytes().unwrap());
    }
}
