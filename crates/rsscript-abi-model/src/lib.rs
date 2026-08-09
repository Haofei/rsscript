#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Version of the provider/runtime semantic call ABI.
pub const RUNTIME_ABI_VERSION: u32 = 2;
/// Version of the deterministic Core library contract used by bytecode.
///
/// This deliberately changes independently from the Provider/runtime ABI:
/// moving a pure builtin or changing its observable semantics must not be
/// mistaken for a host-call compatibility change.
pub const CORE_LIBRARY_ABI_VERSION: u32 = 1;
/// Language semantics carried by compiled artifacts and neutral analysis.
/// This deliberately does not track any crate/package release version.
pub const LANGUAGE_SEMANTICS_VERSION: &str = "0.1.0";

/// Canonical, serializable type representation used by artifacts and Providers.
/// Semantic arenas may use local IDs internally; those IDs never cross the ABI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireType {
    Unit,
    Bool,
    Int {
        bits: u16,
        signed: bool,
    },
    Float {
        bits: u16,
    },
    String,
    Bytes,
    List {
        element: Box<WireType>,
    },
    Option {
        value: Box<WireType>,
    },
    Result {
        ok: Box<WireType>,
        error: Box<WireType>,
    },
    Tuple {
        elements: Vec<WireType>,
    },
    Named {
        package: Option<String>,
        name: String,
        arguments: Vec<WireType>,
    },
    Resource {
        name: String,
    },
    Handle {
        name: String,
    },
    Qualified {
        qualifier: WireQualifier,
        value: Box<WireType>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireQualifier {
    Fresh,
    Owned,
    NoEscape,
}

impl WireType {
    pub fn parse(source: &str) -> Self {
        let source = source.trim();
        for (prefix, qualifier) in [
            ("fresh ", WireQualifier::Fresh),
            ("owned ", WireQualifier::Owned),
            ("noescape ", WireQualifier::NoEscape),
        ] {
            if let Some(value) = source.strip_prefix(prefix) {
                return Self::Qualified {
                    qualifier,
                    value: Box::new(Self::parse(value)),
                };
            }
        }
        match source {
            "Unit" => return Self::Unit,
            "Bool" => return Self::Bool,
            "Int" => {
                return Self::Int {
                    bits: 64,
                    signed: true,
                };
            }
            "Float" => return Self::Float { bits: 64 },
            "String" => return Self::String,
            "Bytes" => return Self::Bytes,
            _ => {}
        }
        if source.starts_with('(') && source.ends_with(')') {
            return Self::Tuple {
                elements: split_type_arguments(&source[1..source.len() - 1])
                    .into_iter()
                    .map(Self::parse)
                    .collect(),
            };
        }
        let (root, arguments) = split_generic(source);
        let arguments = arguments
            .map(split_type_arguments)
            .unwrap_or_default()
            .into_iter()
            .map(Self::parse)
            .collect::<Vec<_>>();
        match (root, arguments.as_slice()) {
            ("List", [element]) => Self::List {
                element: Box::new(element.clone()),
            },
            ("Option", [value]) => Self::Option {
                value: Box::new(value.clone()),
            },
            ("Result", [ok, error]) => Self::Result {
                ok: Box::new(ok.clone()),
                error: Box::new(error.clone()),
            },
            ("Resource", []) => Self::Resource {
                name: "Resource".into(),
            },
            ("Handle", []) => Self::Handle {
                name: "Handle".into(),
            },
            _ => {
                let (package, name) = root.rsplit_once('.').map_or_else(
                    || (None, root.to_string()),
                    |(package, name)| (Some(package.to_string()), name.to_string()),
                );
                Self::Named {
                    package,
                    name,
                    arguments,
                }
            }
        }
    }

    fn encode_canonical(&self, output: &mut Vec<u8>) {
        let encoded = serde_json::to_vec(self).expect("WireType serialization cannot fail");
        output.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
        output.extend_from_slice(&encoded);
    }
}

impl From<&str> for WireType {
    fn from(value: &str) -> Self {
        Self::parse(value)
    }
}

impl From<String> for WireType {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

fn split_generic(source: &str) -> (&str, Option<&str>) {
    source
        .find('<')
        .filter(|_| source.ends_with('>'))
        .map_or((source, None), |start| {
            (&source[..start], Some(&source[start + 1..source.len() - 1]))
        })
}

fn split_type_arguments(source: &str) -> Vec<&str> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut arguments = Vec::new();
    for (index, character) in source.char_indices() {
        match character {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(source[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = source[start..].trim();
    if !tail.is_empty() {
        arguments.push(tail);
    }
    arguments
}

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
    pub ty: WireType,
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    pub parameters: Vec<ParameterSignature>,
    pub result: WireType,
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
        self.result.encode_canonical(&mut canonical);
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
            parameter.ty.encode_canonical(&mut canonical);
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
    /// Canonical structural ABI retained in the artifact so verification and
    /// inspection do not need compiler-owned type strings or Provider metadata.
    pub signature: FunctionSignature,
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
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
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

    #[test]
    fn wire_types_parse_nested_structure_without_textual_abi_fields() {
        assert_eq!(
            WireType::parse("Result<List<String>, host.errors.Failure>"),
            WireType::Result {
                ok: Box::new(WireType::List {
                    element: Box::new(WireType::String),
                }),
                error: Box::new(WireType::Named {
                    package: Some("host.errors".into()),
                    name: "Failure".into(),
                    arguments: vec![],
                }),
            }
        );
        assert_eq!(
            WireType::parse("fresh List<Int>"),
            WireType::Qualified {
                qualifier: WireQualifier::Fresh,
                value: Box::new(WireType::List {
                    element: Box::new(WireType::Int {
                        bits: 64,
                        signed: true,
                    }),
                }),
            }
        );
    }

    #[test]
    fn parameter_names_remain_part_of_named_argument_abi() {
        let mut renamed = signature(DataEffect::Read);
        renamed.parameters[0].name = "text".into();
        assert_ne!(signature(DataEffect::Read).hash(), renamed.hash());
    }
}
