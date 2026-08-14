#![forbid(unsafe_code)]

//! Versioned, provider-neutral Artifact Bundle envelopes.
//!
//! This crate owns the persisted Bundle contract. SDK, CLI, runner, review and
//! inspection tools consume it without depending on one another.

use std::error::Error;
use std::fmt;

use rsscript_abi_model::ExternalImport;
use rsscript_bytecode::BytecodeArtifact;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod semantic_diff;

pub use semantic_diff::{
    ArtifactIdentityV1, AwaitFactV1, CallEdgeFactV1, ChangedFactV1, CountChangeV1,
    DiagnosticFactV1, ExportFactV1, ExternalCallFactV1, ExternalContractFactV1, FactSetDiffV1,
    FunctionParameterFactV1, ResourceLifetimeFactV1, ResourceTransferFactV1, SEMANTIC_DIFF_SCHEMA,
    SemanticDiffV1, TaskGroupFactV1,
};

pub const ARTIFACT_BUNDLE_SCHEMA: &str = "rsscript.artifact_bundle.v1";
pub const ARTIFACT_BUNDLE_MAGIC: &[u8; 8] = b"RSSBND\0\x01";
pub const SOURCE_ANALYSIS_SCHEMA: &str = "rsscript.source_analysis.v1";
pub const PACKAGE_ANALYSIS_SCHEMA: &str = "rsscript.package_analysis.v1";
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
        });
        Self {
            schema: AnalysisSchemaV1::Source,
            payload,
            source: Some(source),
        }
    }

    /// Decode analysis evidence read from a persisted Bundle or produced by a
    /// legacy package-analysis adapter.
    ///
    /// Package analysis remains JSON-shaped during its migration window, but
    /// source analysis is decoded through [`SourceAnalysisV1`] so malformed or
    /// silently extended source evidence cannot enter a verified bundle.
    pub fn from_json(payload: Value) -> Result<Self, ArtifactBundleError> {
        let schema = payload
            .get("$schema")
            .and_then(Value::as_str)
            .ok_or(ArtifactBundleError::MissingAnalysisSchema)?;
        let schema = AnalysisSchemaV1::parse(schema)
            .ok_or_else(|| ArtifactBundleError::UnsupportedAnalysisSchema(schema.to_string()))?;
        if schema == AnalysisSchemaV1::Source {
            let source = SourceAnalysisV1::from_json(payload)?;
            return Ok(Self::source(source));
        }
        Ok(Self {
            schema,
            payload,
            source: None,
        })
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
        }
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
        let analysis_bytes = canonical_json(analysis.payload())?;
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
        validate_v1_json_encoding(&manifest, &manifest_bytes, ArtifactBundleSection::Manifest)?;
        let artifact = take(&mut input, artifact_len)?.to_vec();
        let analysis_bytes = take(&mut input, analysis_len)?.to_vec();
        if !input.is_empty() {
            return Err(ArtifactBundleError::TrailingBytes);
        }
        let analysis: Value = serde_json::from_slice(&analysis_bytes)
            .map_err(|error| ArtifactBundleError::Analysis(error.to_string()))?;
        let analysis = AnalysisEnvelopeV1::from_json(analysis)?;
        validate_v1_json_encoding(
            analysis.payload(),
            &analysis_bytes,
            ArtifactBundleSection::Analysis,
        )?;
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
        let manifest = canonical_json(&self.manifest)?;
        let analysis = canonical_json(self.analysis.payload())?;
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
fn validate_v1_json_encoding(
    value: &impl Serialize,
    bytes: &[u8],
    section: ArtifactBundleSection,
) -> Result<(), ArtifactBundleError> {
    if canonical_json(value)? == bytes || legacy_compact_json(value)? == bytes {
        return Ok(());
    }
    Err(match section {
        ArtifactBundleSection::Manifest => ArtifactBundleError::NonCanonicalManifest,
        ArtifactBundleSection::Analysis => ArtifactBundleError::NonCanonicalAnalysis,
    })
}

fn legacy_compact_json(value: &impl Serialize) -> Result<Vec<u8>, ArtifactBundleError> {
    serde_json::to_vec(value).map_err(|error| ArtifactBundleError::Manifest(error.to_string()))
}

#[derive(Clone, Copy)]
enum ArtifactBundleSection {
    Manifest,
    Analysis,
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
}
