use std::error::Error;
use std::fmt;

use rsscript_abi_model::ExternalImport;
use rsscript_bytecode::BytecodeArtifact;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ARTIFACT_BUNDLE_SCHEMA: &str = "rsscript.artifact_bundle.v1";
pub const ARTIFACT_BUNDLE_MAGIC: &[u8; 8] = b"RSSBND\0\x01";
/// Analysis schemas accepted as provider-neutral evidence by this bundle
/// format. New schemas require an explicit compatibility decision here.
pub const SOURCE_ANALYSIS_SCHEMA: &str = "rsscript.source_analysis.v1";
pub const PACKAGE_ANALYSIS_SCHEMA: &str = "rsscript.package_analysis.v1";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ANALYSIS_BYTES: usize = 16 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildProvenanceV1 {
    pub compiler_version: String,
    pub language_version: String,
    pub core_library_abi_version: u32,
    pub runtime_abi_version: u32,
    pub interface_catalog_digest: String,
    pub source_content_hash: String,
    /// Digest of the immutable source/interface input captured for this
    /// bundle. A deployable bundle never has an identity-less build phase.
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

/// A provider-neutral deployable unit containing executable bytes, neutral
/// semantic analysis, provenance, and exact interface requirements.
#[derive(Debug, Clone)]
pub struct ArtifactBundle {
    manifest: BundleManifestV1,
    artifact: Vec<u8>,
    analysis: serde_json::Value,
    digest: String,
}

impl ArtifactBundle {
    pub(crate) fn new(
        artifact: Vec<u8>,
        analysis: serde_json::Value,
    ) -> Result<Self, ArtifactBundleError> {
        let envelope = BytecodeArtifact::from_bytes(&artifact)
            .map_err(|error| ArtifactBundleError::Artifact(error.to_string()))?;
        let analysis_bytes = canonical_json(&analysis)?;
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
        Self::from_sections(manifest, artifact, analysis, analysis_bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ArtifactBundleError> {
        let mut input = bytes
            .strip_prefix(ARTIFACT_BUNDLE_MAGIC)
            .ok_or(ArtifactBundleError::InvalidMagic)?;
        let manifest_len = take_length(&mut input, MAX_MANIFEST_BYTES)?;
        let artifact_len = take_length(&mut input, MAX_ARTIFACT_BYTES)?;
        let analysis_len = take_length(&mut input, MAX_ANALYSIS_BYTES)?;
        let manifest_bytes = take(&mut input, manifest_len)?;
        let artifact = take(&mut input, artifact_len)?.to_vec();
        let analysis_bytes = take(&mut input, analysis_len)?.to_vec();
        if !input.is_empty() {
            return Err(ArtifactBundleError::TrailingBytes);
        }
        let manifest: BundleManifestV1 = serde_json::from_slice(manifest_bytes)
            .map_err(|error| ArtifactBundleError::Manifest(error.to_string()))?;
        let analysis: serde_json::Value = serde_json::from_slice(&analysis_bytes)
            .map_err(|error| ArtifactBundleError::Analysis(error.to_string()))?;
        if canonical_json(&analysis)? != analysis_bytes {
            return Err(ArtifactBundleError::NonCanonicalAnalysis);
        }
        Self::from_sections(manifest, artifact, analysis, analysis_bytes)
    }

    fn from_sections(
        manifest: BundleManifestV1,
        artifact: Vec<u8>,
        analysis: serde_json::Value,
        analysis_bytes: Vec<u8>,
    ) -> Result<Self, ArtifactBundleError> {
        if manifest.schema != ARTIFACT_BUNDLE_SCHEMA {
            return Err(ArtifactBundleError::UnsupportedSchema(manifest.schema));
        }
        verify_analysis_schema(&analysis)?;
        if manifest.artifact_digest != digest(&artifact) {
            return Err(ArtifactBundleError::ArtifactDigestMismatch);
        }
        if manifest.analysis_digest != digest(&analysis_bytes) {
            return Err(ArtifactBundleError::AnalysisDigestMismatch);
        }
        let envelope = BytecodeArtifact::from_bytes(&artifact)
            .map_err(|error| ArtifactBundleError::Artifact(error.to_string()))?;
        let expected_requirements = envelope
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
            || manifest.required_interfaces != expected_requirements
        {
            return Err(ArtifactBundleError::ManifestArtifactMismatch);
        }
        let manifest_bytes = canonical_json(&manifest)?;
        let digest = bundle_digest(&manifest_bytes, &artifact, &analysis_bytes);
        Ok(Self {
            manifest,
            artifact,
            analysis,
            digest,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ArtifactBundleError> {
        let manifest = canonical_json(&self.manifest)?;
        let analysis = canonical_json(&self.analysis)?;
        let mut output = Vec::with_capacity(
            ARTIFACT_BUNDLE_MAGIC.len()
                + 24
                + manifest.len()
                + self.artifact.len()
                + analysis.len(),
        );
        output.extend_from_slice(ARTIFACT_BUNDLE_MAGIC);
        put_length(&mut output, manifest.len())?;
        put_length(&mut output, self.artifact.len())?;
        put_length(&mut output, analysis.len())?;
        output.extend_from_slice(&manifest);
        output.extend_from_slice(&self.artifact);
        output.extend_from_slice(&analysis);
        Ok(output)
    }

    pub fn artifact_bytes(&self) -> &[u8] {
        &self.artifact
    }

    pub fn analysis(&self) -> &serde_json::Value {
        &self.analysis
    }

    pub fn provenance(&self) -> &BuildProvenanceV1 {
        &self.manifest.provenance
    }

    pub fn required_interfaces(&self) -> &[InterfaceRequirementV1] {
        &self.manifest.required_interfaces
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

fn verify_analysis_schema(analysis: &serde_json::Value) -> Result<(), ArtifactBundleError> {
    let schema = analysis
        .get("$schema")
        .and_then(serde_json::Value::as_str)
        .ok_or(ArtifactBundleError::MissingAnalysisSchema)?;
    if [SOURCE_ANALYSIS_SCHEMA, PACKAGE_ANALYSIS_SCHEMA].contains(&schema) {
        Ok(())
    } else {
        Err(ArtifactBundleError::UnsupportedAnalysisSchema(
            schema.to_string(),
        ))
    }
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, ArtifactBundleError> {
    serde_json::to_vec(value).map_err(|error| ArtifactBundleError::Manifest(error.to_string()))
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn bundle_digest(manifest: &[u8], artifact: &[u8], analysis: &[u8]) -> String {
    let mut hasher = Sha256::new();
    for section in [manifest, artifact, analysis] {
        hasher.update((section.len() as u64).to_be_bytes());
        hasher.update(section);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn put_length(output: &mut Vec<u8>, length: usize) -> Result<(), ArtifactBundleError> {
    let length = u64::try_from(length).map_err(|_| ArtifactBundleError::LengthOverflow)?;
    output.extend_from_slice(&length.to_be_bytes());
    Ok(())
}

fn take_length(input: &mut &[u8], maximum: usize) -> Result<usize, ArtifactBundleError> {
    let bytes: [u8; 8] = take(input, 8)?
        .try_into()
        .map_err(|_| ArtifactBundleError::Truncated)?;
    let length = usize::try_from(u64::from_be_bytes(bytes))
        .map_err(|_| ArtifactBundleError::LengthOverflow)?;
    if length > maximum {
        return Err(ArtifactBundleError::SectionTooLarge { length, maximum });
    }
    Ok(length)
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
    NonCanonicalAnalysis,
    ArtifactDigestMismatch,
    AnalysisDigestMismatch,
    ManifestArtifactMismatch,
}

impl fmt::Display for ArtifactBundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => formatter.write_str("invalid RSScript Artifact Bundle magic"),
            Self::Truncated => formatter.write_str("truncated RSScript Artifact Bundle"),
            Self::TrailingBytes => formatter.write_str("Artifact Bundle has trailing bytes"),
            Self::LengthOverflow => formatter.write_str("Artifact Bundle section length overflow"),
            Self::SectionTooLarge { length, maximum } => write!(
                formatter,
                "Artifact Bundle section is too large ({length} bytes; maximum {maximum})"
            ),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported Artifact Bundle schema `{schema}`")
            }
            Self::MissingAnalysisSchema => {
                formatter.write_str("bundle analysis has no declared schema")
            }
            Self::MissingSnapshotDigest => {
                formatter.write_str("bundle artifact has no immutable snapshot digest")
            }
            Self::UnsupportedAnalysisSchema(schema) => {
                write!(formatter, "unsupported bundle analysis schema `{schema}`")
            }
            Self::Manifest(message) => write!(formatter, "invalid bundle manifest: {message}"),
            Self::Analysis(message) => write!(formatter, "invalid bundle analysis: {message}"),
            Self::Artifact(message) => write!(formatter, "invalid bundled artifact: {message}"),
            Self::NonCanonicalAnalysis => {
                formatter.write_str("bundle analysis is not canonically encoded")
            }
            Self::ArtifactDigestMismatch => formatter.write_str("artifact digest mismatch"),
            Self::AnalysisDigestMismatch => formatter.write_str("analysis digest mismatch"),
            Self::ManifestArtifactMismatch => {
                formatter.write_str("bundle manifest does not match the bytecode artifact")
            }
        }
    }
}

impl Error for ArtifactBundleError {}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_bytecode::BytecodeArtifact;

    const SNAPSHOT_DIGEST: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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
        artifact.bind_snapshot_digest(SNAPSHOT_DIGEST).unwrap();
        let artifact = artifact.to_bytes().unwrap();
        ArtifactBundle::new(
            artifact,
            serde_json::json!({"$schema": SOURCE_ANALYSIS_SCHEMA}),
        )
        .unwrap()
    }

    #[test]
    fn round_trip_binds_all_sections() {
        let original = bundle();
        let decoded = ArtifactBundle::from_bytes(&original.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded.digest(), original.digest());
        assert_eq!(decoded.analysis(), original.analysis());
        assert_eq!(decoded.artifact_bytes(), original.artifact_bytes());
    }

    #[test]
    fn tampering_fails_closed() {
        let original = bundle();
        let mut bytes = original.to_bytes().unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 1;
        assert!(ArtifactBundle::from_bytes(&bytes).is_err());
    }

    #[test]
    fn analysis_schema_is_an_explicit_fail_closed_compatibility_boundary() {
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
        artifact.bind_snapshot_digest(SNAPSHOT_DIGEST).unwrap();
        let artifact = artifact.to_bytes().unwrap();
        assert!(matches!(
            ArtifactBundle::new(artifact, serde_json::json!({"$schema": "future.analysis.v1"})),
            Err(ArtifactBundleError::UnsupportedAnalysisSchema(schema)) if schema == "future.analysis.v1"
        ));
    }

    #[test]
    fn deployable_bundle_rejects_bytecode_without_a_snapshot_identity() {
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            "sha256:catalog",
            2,
            "sha256:source",
            vec![],
            vec![1, 2, 3],
        )
        .unwrap()
        .to_bytes()
        .unwrap();
        assert!(matches!(
            ArtifactBundle::new(
                artifact,
                serde_json::json!({"$schema": SOURCE_ANALYSIS_SCHEMA}),
            ),
            Err(ArtifactBundleError::MissingSnapshotDigest)
        ));
    }
}
