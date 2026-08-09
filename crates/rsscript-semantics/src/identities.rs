use serde::{Deserialize, Serialize};

/// Stable identity of a checked declaration in one semantic database.
///
/// It is deliberately distinct from source/module/type IDs so downstream
/// queries cannot accidentally use one namespace in place of another.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct DefinitionId(u32);

impl DefinitionId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[cfg(test)]
mod tests {
    use super::DefinitionId;

    #[test]
    fn definition_identity_has_a_stable_wire_form() {
        let id = DefinitionId::new(42);
        assert_eq!(id.index(), 42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(serde_json::from_str::<DefinitionId>("42").unwrap(), id);
    }
}
