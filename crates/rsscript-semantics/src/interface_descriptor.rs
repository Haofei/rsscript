use rsscript_syntax::ast::{DataEffect as SyntaxEffect, Item, TypeRef};
use rsscript_syntax::parse_source;
use serde::Serialize;

use crate::{DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature};

pub const INTERFACE_DESCRIPTOR_SCHEMA: &str = "rsscript.interface_descriptor.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterfaceDescriptorFunctionV1 {
    pub symbol: ExternalSymbol,
    pub entry: String,
    pub signature: FunctionSignature,
}

/// Canonical semantic description of bodyless `.rssi` function contracts.
/// Provider bindgen consumes this descriptor rather than reparsing source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterfaceDescriptorV1 {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub functions: Vec<InterfaceDescriptorFunctionV1>,
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
        for item in program.items {
            let Item::Function(function) = item else {
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
            functions.push(InterfaceDescriptorFunctionV1 {
                symbol,
                entry,
                signature: FunctionSignature {
                    parameters,
                    result: result.into(),
                    asynchronous: function.is_async,
                },
            });
        }
        functions.sort_by(|left, right| left.symbol.cmp(&right.symbol));
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
        })
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
            "module host.log\npub async fn emit(value: take owned String) -> fresh Unit retains(value)\n",
        )
        .expect("valid interface");
        assert_eq!(descriptor.schema, INTERFACE_DESCRIPTOR_SCHEMA);
        assert_eq!(descriptor.functions[0].symbol.as_str(), "host.log.emit");
        assert!(descriptor.functions[0].signature.asynchronous);
        assert!(descriptor.functions[0].signature.parameters[0].retained);
        assert_eq!(
            descriptor.functions[0].signature.parameters[0].effect,
            DataEffect::Take
        );
    }
}
