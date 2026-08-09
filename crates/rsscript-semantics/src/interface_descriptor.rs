use rsscript_syntax::ast::{DataEffect as SyntaxEffect, Item, TypeKind, TypeRef};
use rsscript_syntax::parse_source;
use serde::Serialize;

use crate::{DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, SignatureHash};

pub const INTERFACE_DESCRIPTOR_SCHEMA: &str = "rsscript.interface_descriptor.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterfaceDescriptorFunctionV1 {
    pub symbol: ExternalSymbol,
    pub entry: String,
    pub signature: FunctionSignature,
    pub signature_hash: SignatureHash,
}

/// Public resource type exposed by an interface. Cleanup implementation stays
/// provider-owned; this descriptor records only the language-visible contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterfaceDescriptorResourceV1 {
    pub name: String,
    pub opaque: bool,
    pub type_parameters: Vec<String>,
}

/// Canonical semantic description of bodyless `.rssi` function contracts.
/// Provider bindgen consumes this descriptor rather than reparsing source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterfaceDescriptorV1 {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub functions: Vec<InterfaceDescriptorFunctionV1>,
    pub resources: Vec<InterfaceDescriptorResourceV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceDescriptorError {
    MalformedInterface,
    MissingModule,
    InvalidSymbol(String),
    DuplicateSymbol(String),
}

impl InterfaceDescriptorV1 {
    pub fn from_interface_source(
        path: &str,
        source: &str,
    ) -> Result<Self, InterfaceDescriptorError> {
        let program = parse_source(path, source);
        if !program.unknown_top_level_spans.is_empty()
            || !program.malformed_declaration_spans.is_empty()
        {
            return Err(InterfaceDescriptorError::MalformedInterface);
        }
        let module = program.items.iter().find_map(|item| match item {
            Item::Module(module) => Some(module.path.join(".")),
            _ => None,
        });
        let mut functions = Vec::new();
        let mut resources = Vec::new();
        for item in program.items {
            let Item::Function(function) = item else {
                if let Item::Type(resource) = item
                    && resource.kind == TypeKind::Resource
                    && resource.is_public
                {
                    resources.push(InterfaceDescriptorResourceV1 {
                        name: resource.name,
                        opaque: resource.is_opaque,
                        type_parameters: resource
                            .type_params
                            .into_iter()
                            .map(|parameter| parameter.name)
                            .collect(),
                    });
                }
                continue;
            };
            if function.has_body || !function.malformed_param_spans.is_empty() {
                return Err(InterfaceDescriptorError::MalformedInterface);
            }
            let symbol_text = if function.name.contains('.') {
                function.name.clone()
            } else {
                format!(
                    "{}.{}",
                    module
                        .as_deref()
                        .ok_or(InterfaceDescriptorError::MissingModule)?,
                    function.name
                )
            };
            let symbol = ExternalSymbol::new(symbol_text.clone())
                .map_err(|_| InterfaceDescriptorError::InvalidSymbol(symbol_text))?;
            let entry = function
                .lower_name
                .clone()
                .unwrap_or_else(|| function.name.rsplit('.').next().unwrap().to_string());
            let parameters = function
                .params
                .iter()
                .map(|parameter| ParameterSignature {
                    name: parameter.name.clone(),
                    effect: match parameter.effective_effect().unwrap_or(SyntaxEffect::Read) {
                        SyntaxEffect::Read => DataEffect::Read,
                        SyntaxEffect::Mut => DataEffect::Mut,
                        SyntaxEffect::Take => DataEffect::Take,
                    },
                    ty: type_name(&parameter.ty).into(),
                    retained: function.retained_params.contains(&parameter.name),
                })
                .collect();
            let mut result = function
                .return_ty
                .as_ref()
                .map(type_name)
                .unwrap_or_else(|| "Unit".to_string());
            if function.returns_fresh && !result.starts_with("fresh ") {
                result = format!("fresh {result}");
            }
            let signature = FunctionSignature {
                parameters,
                result: result.into(),
                asynchronous: function.is_async,
            };
            functions.push(InterfaceDescriptorFunctionV1 {
                symbol,
                entry,
                signature_hash: signature.hash(),
                signature,
            });
        }
        functions.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        resources.sort_by(|left, right| left.name.cmp(&right.name));
        if let Some(pair) = functions
            .windows(2)
            .find(|pair| pair[0].symbol == pair[1].symbol)
        {
            return Err(InterfaceDescriptorError::DuplicateSymbol(
                pair[0].symbol.as_str().to_string(),
            ));
        }
        Ok(Self {
            schema: INTERFACE_DESCRIPTOR_SCHEMA.to_string(),
            functions,
            resources,
        })
    }

    /// Canonical descriptor bytes for snapshots, generated bindings, and ABI
    /// drift checks. Every field is ordered structurally and functions are
    /// sorted by canonical external symbol before this is produced.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

fn type_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let parameters = ty
            .fn_params
            .iter()
            .map(type_name)
            .collect::<Vec<_>>()
            .join(", ");
        let result = ty
            .fn_return
            .as_ref()
            .map(|result| format!(" -> {}", type_name(result)))
            .unwrap_or_default();
        format!("Fn({parameters}){result}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args.iter().map(type_name).collect::<Vec<_>>().join(", ")
        )
    };
    let qualified = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {qualified}")
    } else {
        qualified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_canonicalizes_interface_contracts_once() {
        let descriptor = InterfaceDescriptorV1::from_interface_source(
            "host.rssi",
            "module host.log\npub resource Stream\npub async fn emit(value: take owned String) -> fresh Unit retains(value)\n",
        )
        .expect("valid interface");
        assert_eq!(descriptor.schema, INTERFACE_DESCRIPTOR_SCHEMA);
        assert_eq!(descriptor.functions[0].symbol.as_str(), "host.log.emit");
        assert_eq!(descriptor.resources[0].name, "Stream");
        assert!(descriptor.functions[0].signature.asynchronous);
        assert!(descriptor.functions[0].signature.parameters[0].retained);
        assert_eq!(
            descriptor.functions[0].signature.parameters[0].effect,
            DataEffect::Take
        );
        assert_eq!(
            descriptor.functions[0].signature_hash,
            descriptor.functions[0].signature.hash()
        );
        assert_eq!(
            descriptor.to_json_bytes().unwrap(),
            descriptor.to_json_bytes().unwrap()
        );
    }
}
