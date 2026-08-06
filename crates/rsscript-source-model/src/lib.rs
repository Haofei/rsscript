#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceRevision(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub length: usize,
}

impl TextRange {
    pub fn end(self) -> Option<usize> {
        self.start.checked_add(self.length)
    }
}

/// Human-readable source coordinate retained by the public diagnostic schema.
/// `FileId`/`TextRange` are used by revisioned internal indexes; this projection
/// stays stable for serialized diagnostics and existing embedding callers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_range_end_is_overflow_checked() {
        assert_eq!(
            TextRange {
                start: 4,
                length: 7
            }
            .end(),
            Some(11)
        );
        assert_eq!(
            TextRange {
                start: usize::MAX,
                length: 1
            }
            .end(),
            None
        );
    }
}
