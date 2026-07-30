use std::fmt;
use std::io;

use crate::FrameKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    field: String,
    message: String,
}

impl ValidationError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    Truncated {
        section: &'static str,
    },
    BadMagic {
        actual: [u8; 4],
    },
    UnsupportedVersion {
        actual: u16,
    },
    UnexpectedKind {
        expected: FrameKind,
        actual: u16,
    },
    PayloadTooLarge {
        kind: FrameKind,
        actual: u32,
        limit: usize,
    },
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    Validation(ValidationError),
    TrailingData {
        bytes: usize,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "worker protocol I/O failed: {error}"),
            Self::Truncated { section } => {
                write!(f, "worker protocol frame has a truncated {section}")
            }
            Self::BadMagic { actual } => {
                write!(f, "worker protocol magic is invalid: {actual:02x?}")
            }
            Self::UnsupportedVersion { actual } => {
                write!(f, "worker protocol version {actual} is unsupported")
            }
            Self::UnexpectedKind { expected, actual } => write!(
                f,
                "worker protocol expected {} frame, got kind {actual}",
                expected.name()
            ),
            Self::PayloadTooLarge {
                kind,
                actual,
                limit,
            } => write!(
                f,
                "worker protocol {} payload is {actual} bytes, limit is {limit}",
                kind.name()
            ),
            Self::Serialize(error) => {
                write!(f, "worker protocol payload serialization failed: {error}")
            }
            Self::Deserialize(error) => {
                write!(f, "worker protocol payload decoding failed: {error}")
            }
            Self::Validation(error) => write!(f, "worker protocol validation failed: {error}"),
            Self::TrailingData { bytes } => {
                write!(f, "worker protocol frame has {bytes} trailing bytes")
            }
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
            Self::Validation(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ValidationError> for ProtocolError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}
