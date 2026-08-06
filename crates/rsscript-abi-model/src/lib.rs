#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalSymbol(String);

impl ExternalSymbol {
    pub fn new(symbol: impl Into<String>) -> Result<Self, InvalidExternalSymbol> {
        let symbol = symbol.into();
        if symbol.is_empty()
            || symbol.starts_with('.')
            || symbol.ends_with('.')
            || symbol.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
        {
            return Err(InvalidExternalSymbol);
        }
        Ok(Self(symbol))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExternalSymbol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidExternalSymbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataEffect {
    Read,
    Mut,
    Take,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterSignature {
    pub name: String,
    pub effect: DataEffect,
    pub type_name: String,
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    pub parameters: Vec<ParameterSignature>,
    pub return_type: String,
    pub asynchronous: bool,
}

impl FunctionSignature {
    pub fn hash(&self) -> SignatureHash {
        let mut canonical = Vec::new();
        canonical.extend_from_slice(b"rsscript.semantic_signature.v1\0");
        append_field(
            &mut canonical,
            if self.asynchronous { "async" } else { "sync" },
        );
        append_field(&mut canonical, &self.return_type);
        canonical.extend_from_slice(&(self.parameters.len() as u64).to_be_bytes());
        for parameter in &self.parameters {
            append_field(&mut canonical, &parameter.name);
            append_field(
                &mut canonical,
                match parameter.effect {
                    DataEffect::Read => "read",
                    DataEffect::Mut => "mut",
                    DataEffect::Take => "take",
                },
            );
            append_field(&mut canonical, &parameter.type_name);
            canonical.push(u8::from(parameter.retained));
        }
        let digest = Sha256::digest(canonical);
        SignatureHash(format!("sha256:{digest:x}"))
    }
}

fn append_field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SignatureHash(String);

impl SignatureHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalImport {
    pub symbol: ExternalSymbol,
    pub signature_hash: SignatureHash,
    pub abi_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(effect: DataEffect) -> FunctionSignature {
        FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "message".to_string(),
                effect,
                type_name: "String".to_string(),
                retained: false,
            }],
            return_type: "Unit".to_string(),
            asynchronous: false,
        }
    }

    #[test]
    fn signature_hash_is_deterministic_and_semantic() {
        assert_eq!(
            signature(DataEffect::Read).hash(),
            signature(DataEffect::Read).hash()
        );
        assert_ne!(
            signature(DataEffect::Read).hash(),
            signature(DataEffect::Take).hash()
        );
    }
}
