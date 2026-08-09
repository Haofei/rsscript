#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

macro_rules! stable_id {
    ($doc:literal, $name:ident, $inner:ty) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[doc = $doc]
        #[serde(transparent)]
        pub struct $name($inner);

        impl $name {
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            pub const fn get(self) -> $inner {
                self.0
            }
        }
    };
}

stable_id!(
    "Stable identity of one file inside an immutable source snapshot.",
    FileId,
    u32
);
stable_id!(
    "Monotonic revision of a file owned by a future compilation session.",
    SourceRevision,
    u64
);
stable_id!(
    "Stable identity of a source module in a compilation session.",
    ModuleId,
    u32
);
stable_id!(
    "Stable identity of an interface module in a compilation session.",
    InterfaceId,
    u32
);

impl SourceRevision {
    pub const INITIAL: Self = Self::new(0);

    /// Produce the next revision without silently wrapping a long-lived session.
    pub const fn next(self) -> Option<Self> {
        match self.get().checked_add(1) {
            Some(value) => Some(Self::new(value)),
            None => None,
        }
    }
}

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
    fn stable_identity_round_trips_through_its_wire_form() {
        let identities = [
            serde_json::to_string(&FileId::new(7)).unwrap(),
            serde_json::to_string(&SourceRevision::new(9)).unwrap(),
            serde_json::to_string(&ModuleId::new(11)).unwrap(),
            serde_json::to_string(&InterfaceId::new(13)).unwrap(),
        ];
        assert_eq!(identities, ["7", "9", "11", "13"]);
        assert_eq!(SourceRevision::INITIAL.next(), Some(SourceRevision::new(1)));
        assert_eq!(SourceRevision::new(u64::MAX).next(), None);
    }

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
