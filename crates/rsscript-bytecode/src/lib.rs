#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;

use rsscript_abi_model::ExternalImport;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BYTECODE_SCHEMA: &str = "rsscript.bytecode.v1";
pub const BYTECODE_MAGIC: &[u8; 8] = b"RSSBC\0\x01\0";
const SECTION_HEADER: u8 = 1;
const SECTION_IMPORTS: u8 = 2;
const SECTION_CODE: u8 = 3;
const SECTION_CHECKSUM: u8 = 4;
const SECTION_REQUIRED: u8 = 1;
const SECTION_HEADER_BYTES: usize = 1 + 1 + 8 + 32;
const MAX_SECTIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
        let header = serde_json::to_vec(&self.header).map_err(BytecodeError::Encode)?;
        let imports = serde_json::to_vec(&self.imports).map_err(BytecodeError::Encode)?;
        let sections = [
            (SECTION_HEADER, header.as_slice()),
            (SECTION_IMPORTS, imports.as_slice()),
            (SECTION_CODE, self.payload.as_slice()),
            (SECTION_CHECKSUM, self.checksum.as_bytes()),
        ];
        let mut bytes = Vec::with_capacity(
            BYTECODE_MAGIC.len()
                + 2
                + sections
                    .iter()
                    .map(|(_, data)| SECTION_HEADER_BYTES + data.len())
                    .sum::<usize>(),
        );
        bytes.extend_from_slice(BYTECODE_MAGIC);
        bytes.extend_from_slice(&(sections.len() as u16).to_be_bytes());
        for (kind, data) in sections {
            bytes.push(kind);
            bytes.push(SECTION_REQUIRED);
            bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
            bytes.extend_from_slice(&Sha256::digest(data));
            bytes.extend_from_slice(data);
        }
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
        let mut body = bytes
            .strip_prefix(BYTECODE_MAGIC)
            .ok_or(BytecodeError::InvalidMagic)?;
        let section_count = take_array::<2>(&mut body)
            .map(u16::from_be_bytes)
            .map(usize::from)?;
        if section_count == 0 || section_count > MAX_SECTIONS {
            return Err(BytecodeError::MalformedSectionTable);
        }
        let mut header = None;
        let mut imports = None;
        let mut payload = None;
        let mut checksum = None;
        let mut previous_kind = 0u8;
        for _ in 0..section_count {
            let kind = take_array::<1>(&mut body)?[0];
            let flags = take_array::<1>(&mut body)?[0];
            if flags & !SECTION_REQUIRED != 0 {
                return Err(BytecodeError::InvalidSectionFlags { kind, flags });
            }
            let length = usize::try_from(u64::from_be_bytes(take_array::<8>(&mut body)?))
                .map_err(|_| BytecodeError::MalformedSectionTable)?;
            let expected_hash = take_array::<32>(&mut body)?;
            let data = take_bytes(&mut body, length)?;
            if Sha256::digest(data).as_slice() != expected_hash {
                return Err(BytecodeError::SectionHashMismatch(kind));
            }
            if kind <= previous_kind {
                return Err(BytecodeError::SectionsNotCanonical);
            }
            previous_kind = kind;
            match kind {
                SECTION_HEADER => {
                    require_section(kind, flags)?;
                    let decoded: BytecodeHeader =
                        serde_json::from_slice(data).map_err(BytecodeError::Decode)?;
                    require_canonical_json(data, &decoded)?;
                    header = Some(decoded);
                }
                SECTION_IMPORTS => {
                    require_section(kind, flags)?;
                    let decoded: Vec<ExternalImport> =
                        serde_json::from_slice(data).map_err(BytecodeError::Decode)?;
                    require_canonical_json(data, &decoded)?;
                    imports = Some(decoded);
                }
                SECTION_CODE => {
                    require_section(kind, flags)?;
                    payload = Some(data.to_vec());
                }
                SECTION_CHECKSUM => {
                    require_section(kind, flags)?;
                    checksum = Some(
                        std::str::from_utf8(data)
                            .map_err(|_| BytecodeError::MalformedChecksum)?
                            .to_string(),
                    );
                }
                unknown if flags & SECTION_REQUIRED != 0 => {
                    return Err(BytecodeError::UnknownRequiredSection(unknown));
                }
                _ => {}
            }
        }
        if !body.is_empty() {
            return Err(BytecodeError::TrailingBytes);
        }
        Ok(Self {
            header: header.ok_or(BytecodeError::MissingSection(SECTION_HEADER))?,
            imports: imports.ok_or(BytecodeError::MissingSection(SECTION_IMPORTS))?,
            payload: payload.ok_or(BytecodeError::MissingSection(SECTION_CODE))?,
            checksum: checksum.ok_or(BytecodeError::MissingSection(SECTION_CHECKSUM))?,
        })
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

fn take_array<const N: usize>(input: &mut &[u8]) -> Result<[u8; N], BytecodeError> {
    let bytes = take_bytes(input, N)?;
    bytes
        .try_into()
        .map_err(|_| BytecodeError::MalformedSectionTable)
}

fn take_bytes<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8], BytecodeError> {
    if length > input.len() {
        return Err(BytecodeError::MalformedSectionTable);
    }
    let (value, rest) = input.split_at(length);
    *input = rest;
    Ok(value)
}

fn require_section(kind: u8, flags: u8) -> Result<(), BytecodeError> {
    if flags & SECTION_REQUIRED == 0 {
        return Err(BytecodeError::KnownSectionNotRequired(kind));
    }
    Ok(())
}

fn require_canonical_json<T: Serialize>(bytes: &[u8], value: &T) -> Result<(), BytecodeError> {
    let canonical = serde_json::to_vec(value).map_err(BytecodeError::Encode)?;
    if canonical != bytes {
        return Err(BytecodeError::SectionsNotCanonical);
    }
    Ok(())
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
    MalformedSectionTable,
    SectionsNotCanonical,
    MissingSection(u8),
    UnknownRequiredSection(u8),
    KnownSectionNotRequired(u8),
    InvalidSectionFlags { kind: u8, flags: u8 },
    SectionHashMismatch(u8),
    MalformedChecksum,
    TrailingBytes,
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
            Self::MalformedSectionTable => formatter.write_str("malformed bytecode section table"),
            Self::SectionsNotCanonical => {
                formatter.write_str("bytecode sections are duplicated or not canonical")
            }
            Self::MissingSection(section) => {
                write!(formatter, "bytecode is missing required section {section}")
            }
            Self::UnknownRequiredSection(section) => {
                write!(
                    formatter,
                    "bytecode contains unknown required section {section}"
                )
            }
            Self::KnownSectionNotRequired(section) => {
                write!(
                    formatter,
                    "bytecode section {section} is not marked required"
                )
            }
            Self::InvalidSectionFlags { kind, flags } => {
                write!(
                    formatter,
                    "bytecode section {kind} has invalid flags {flags:#04x}"
                )
            }
            Self::SectionHashMismatch(section) => {
                write!(formatter, "bytecode section {section} hash mismatch")
            }
            Self::MalformedChecksum => formatter.write_str("bytecode checksum is not UTF-8"),
            Self::TrailingBytes => formatter.write_str("bytecode has trailing bytes"),
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

    #[test]
    fn unknown_optional_sections_are_forward_compatible() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], vec![1, 2, 3])
            .expect("artifact");
        let mut bytes = artifact.to_bytes().expect("bytes");
        bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
        append_test_section(&mut bytes, 5, 0, b"future metadata");

        BytecodeVerifier::default()
            .verify(&bytes)
            .expect("optional section should be ignored");
    }

    #[test]
    fn unknown_required_sections_fail_closed() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], vec![1, 2, 3])
            .expect("artifact");
        let mut bytes = artifact.to_bytes().expect("bytes");
        bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
        append_test_section(&mut bytes, 5, SECTION_REQUIRED, b"future semantics");

        assert!(matches!(
            BytecodeVerifier::default().verify(&bytes),
            Err(BytecodeError::UnknownRequiredSection(5))
        ));
    }

    #[test]
    fn noncanonical_json_section_is_rejected() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], vec![1, 2, 3])
            .expect("artifact");
        let bytes = artifact.to_bytes().expect("bytes");
        let header_offset = BYTECODE_MAGIC.len() + 2;
        let data_offset = header_offset + SECTION_HEADER_BYTES;
        let data_length = u64::from_be_bytes(
            bytes[header_offset + 2..header_offset + 10]
                .try_into()
                .expect("section length"),
        ) as usize;
        let mut rewritten = Vec::new();
        rewritten.extend_from_slice(&bytes[..header_offset]);
        let mut header = Vec::with_capacity(data_length + 1);
        header.push(b' ');
        header.extend_from_slice(&bytes[data_offset..data_offset + data_length]);
        append_test_section(&mut rewritten, SECTION_HEADER, SECTION_REQUIRED, &header);
        rewritten.extend_from_slice(&bytes[data_offset + data_length..]);

        assert!(matches!(
            BytecodeVerifier::default().verify(&rewritten),
            Err(BytecodeError::SectionsNotCanonical)
        ));
    }

    fn append_test_section(bytes: &mut Vec<u8>, kind: u8, flags: u8, data: &[u8]) {
        bytes.push(kind);
        bytes.push(flags);
        bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&Sha256::digest(data));
        bytes.extend_from_slice(data);
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
