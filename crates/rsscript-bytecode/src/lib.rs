#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;

use rsscript_abi_model::ExternalImport;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BYTECODE_SCHEMA: &str = "rsscript.bytecode.v1";
pub const BYTECODE_MAGIC: &[u8; 8] = b"RSSBC\0\x01\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytecodeHeader {
    pub schema: String,
    pub language_version: String,
    pub runtime_abi_version: u32,
    pub source_content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_digest: Option<String>,
    pub executable_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytecodeArtifact {
    pub header: BytecodeHeader,
    pub imports: Vec<ExternalImport>,
    pub payload: Vec<u8>,
    pub checksum: String,
}

impl BytecodeArtifact {
    pub fn new(
        language_version: impl Into<String>,
        runtime_abi_version: u32,
        source_content_hash: impl Into<String>,
        mut imports: Vec<ExternalImport>,
        payload: Vec<u8>,
    ) -> Result<Self, BytecodeError> {
        imports.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        let executable_hash = digest(&payload);
        let mut artifact = Self {
            header: BytecodeHeader {
                schema: BYTECODE_SCHEMA.to_string(),
                language_version: language_version.into(),
                runtime_abi_version,
                source_content_hash: source_content_hash.into(),
                snapshot_digest: None,
                executable_hash,
            },
            imports,
            payload,
            checksum: String::new(),
        };
        artifact.checksum = artifact.compute_checksum()?;
        Ok(artifact)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BytecodeError> {
        let mut bytes = BYTECODE_MAGIC.to_vec();
        bytes.extend(serde_json::to_vec(self).map_err(BytecodeError::Encode)?);
        Ok(bytes)
    }

    /// Bind the artifact to the immutable workspace snapshot that produced it.
    /// Recomputes the envelope checksum; the executable payload is unchanged.
    pub fn bind_snapshot_digest(&mut self, digest: impl Into<String>) -> Result<(), BytecodeError> {
        self.header.snapshot_digest = Some(digest.into());
        self.checksum = self.compute_checksum()?;
        Ok(())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BytecodeError> {
        let body = bytes
            .strip_prefix(BYTECODE_MAGIC)
            .ok_or(BytecodeError::InvalidMagic)?;
        serde_json::from_slice(body).map_err(BytecodeError::Decode)
    }

    fn compute_checksum(&self) -> Result<String, BytecodeError> {
        #[derive(Serialize)]
        struct ChecksumInput<'a> {
            header: &'a BytecodeHeader,
            imports: &'a [ExternalImport],
            payload: &'a [u8],
        }
        let input = serde_json::to_vec(&ChecksumInput {
            header: &self.header,
            imports: &self.imports,
            payload: &self.payload,
        })
        .map_err(BytecodeError::Encode)?;
        Ok(digest(&input))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeLimits {
    pub max_artifact_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_imports: usize,
}

impl Default for BytecodeLimits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: 64 * 1024 * 1024,
            max_payload_bytes: 48 * 1024 * 1024,
            max_imports: 16_384,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedBytecode {
    artifact: BytecodeArtifact,
}

impl VerifiedBytecode {
    pub fn artifact(&self) -> &BytecodeArtifact {
        &self.artifact
    }

    pub fn into_artifact(self) -> BytecodeArtifact {
        self.artifact
    }
}

pub struct BytecodeVerifier {
    limits: BytecodeLimits,
}

impl BytecodeVerifier {
    pub fn new(limits: BytecodeLimits) -> Self {
        Self { limits }
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<VerifiedBytecode, BytecodeError> {
        if bytes.len() > self.limits.max_artifact_bytes {
            return Err(BytecodeError::LimitExceeded("artifact bytes"));
        }
        let artifact = BytecodeArtifact::from_bytes(bytes)?;
        if artifact.header.schema != BYTECODE_SCHEMA {
            return Err(BytecodeError::UnsupportedSchema(artifact.header.schema));
        }
        if artifact.payload.len() > self.limits.max_payload_bytes {
            return Err(BytecodeError::LimitExceeded("payload bytes"));
        }
        if artifact.imports.len() > self.limits.max_imports {
            return Err(BytecodeError::LimitExceeded("imports"));
        }
        if artifact.header.executable_hash != digest(&artifact.payload) {
            return Err(BytecodeError::ExecutableHashMismatch);
        }
        if artifact.checksum != artifact.compute_checksum()? {
            return Err(BytecodeError::ChecksumMismatch);
        }
        if artifact
            .imports
            .windows(2)
            .any(|pair| pair[0].symbol >= pair[1].symbol)
        {
            return Err(BytecodeError::ImportsNotCanonical);
        }
        if artifact
            .imports
            .iter()
            .any(|import| import.abi_version != artifact.header.runtime_abi_version)
        {
            return Err(BytecodeError::ImportAbiMismatch);
        }
        Ok(VerifiedBytecode { artifact })
    }
}

impl Default for BytecodeVerifier {
    fn default() -> Self {
        Self::new(BytecodeLimits::default())
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[derive(Debug)]
pub enum BytecodeError {
    InvalidMagic,
    UnsupportedSchema(String),
    LimitExceeded(&'static str),
    ExecutableHashMismatch,
    ChecksumMismatch,
    ImportsNotCanonical,
    ImportAbiMismatch,
    InvalidPayload(String),
    Encode(serde_json::Error),
    Decode(serde_json::Error),
}

impl fmt::Display for BytecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => formatter.write_str("invalid RSScript bytecode magic"),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported bytecode schema `{schema}`")
            }
            Self::LimitExceeded(limit) => {
                write!(formatter, "bytecode {limit} exceeds verifier limit")
            }
            Self::ExecutableHashMismatch => {
                formatter.write_str("bytecode executable hash mismatch")
            }
            Self::ChecksumMismatch => formatter.write_str("bytecode artifact checksum mismatch"),
            Self::ImportsNotCanonical => {
                formatter.write_str("bytecode imports are duplicated or not sorted")
            }
            Self::ImportAbiMismatch => {
                formatter.write_str("bytecode import ABI does not match its header")
            }
            Self::InvalidPayload(message) => {
                write!(formatter, "invalid bytecode payload: {message}")
            }
            Self::Encode(error) => write!(formatter, "cannot encode bytecode: {error}"),
            Self::Decode(error) => write!(formatter, "cannot decode bytecode: {error}"),
        }
    }
}

impl Error for BytecodeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_trip_requires_intact_artifact() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], vec![1, 2, 3])
            .expect("artifact");
        let bytes = artifact.to_bytes().expect("bytes");
        let verified = BytecodeVerifier::default()
            .verify(&bytes)
            .expect("verified");
        assert_eq!(verified.artifact().payload, [1, 2, 3]);

        let mut corrupt = bytes;
        *corrupt.last_mut().expect("non-empty") ^= 1;
        assert!(BytecodeVerifier::default().verify(&corrupt).is_err());
    }

    proptest! {
        #[test]
        fn arbitrary_bounded_input_is_rejected_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let verifier = BytecodeVerifier::new(BytecodeLimits {
                max_artifact_bytes: 2048,
                max_payload_bytes: 1024,
                max_imports: 32,
            });
            let _ = verifier.verify(&bytes);
        }
    }
}
