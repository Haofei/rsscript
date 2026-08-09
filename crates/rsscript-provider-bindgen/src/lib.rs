#![forbid(unsafe_code)]

use rsscript_abi_model::{ExternalSymbol, FunctionSignature, WireQualifier, WireType};
use rsscript_semantics::{InterfaceDescriptorError, InterfaceDescriptorV1};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceFunction {
    pub symbol: ExternalSymbol,
    pub entry: String,
    pub signature: FunctionSignature,
    pub signature_hash: rsscript_abi_model::SignatureHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInterface {
    pub functions: Vec<InterfaceFunction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedBlocking {
    NonBlocking,
    MayBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedCancellation {
    NotApplicable,
    Cooperative,
    AbortSafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedCleanup {
    None,
    ProviderManaged,
    RuntimeRegistered,
}

#[derive(Debug, Clone)]
pub struct RustProviderOptions<'a> {
    pub provider_id: &'a str,
    pub blocking: GeneratedBlocking,
    pub cancellation: GeneratedCancellation,
    pub thread_safe: bool,
    pub reentrant: bool,
    pub cleanup: GeneratedCleanup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindgenError {
    Descriptor(InterfaceDescriptorError),
}

impl fmt::Display for BindgenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "provider interface generation failed: {self:?}")
    }
}

impl Error for BindgenError {}

impl From<InterfaceDescriptorError> for BindgenError {
    fn from(error: InterfaceDescriptorError) -> Self {
        Self::Descriptor(error)
    }
}

impl ProviderInterface {
    pub fn from_descriptor(descriptor: InterfaceDescriptorV1) -> Result<Self, BindgenError> {
        if descriptor.schema != rsscript_semantics::INTERFACE_DESCRIPTOR_SCHEMA {
            return Err(BindgenError::Descriptor(
                InterfaceDescriptorError::MalformedInterface,
            ));
        }
        Ok(Self {
            functions: descriptor
                .functions
                .into_iter()
                .map(|function| InterfaceFunction {
                    symbol: function.symbol,
                    entry: function.entry,
                    signature: function.signature,
                    signature_hash: function.signature_hash,
                })
                .collect(),
        })
    }

    pub fn render_rust(&self, options: &RustProviderOptions<'_>) -> String {
        let mut output = String::from("// @generated from .rssi; do not edit.\n");
        output.push_str("pub trait GeneratedProviderContract {\n");
        for function in &self.functions {
            output.push_str(&format!(
                "    fn {}(&self, args: Vec<rsscript_provider_api::NativeValue>) -> Result<rsscript_provider_api::NativeValue, rsscript_provider_api::ProviderError>;\n",
                rust_identifier(&function.entry)
            ));
        }
        output
            .push_str("}\n\npub fn descriptor() -> rsscript_provider_api::ProviderDescriptor {\n");
        output.push_str(&format!(
            "    rsscript_provider_api::ProviderDescriptor {{ provider_id: {:?}.into(), provider_version: env!(\"CARGO_PKG_VERSION\").into(), supported_abi: vec![rsscript_abi_model::RUNTIME_ABI_VERSION], functions: vec![\n",
            options.provider_id
        ));
        for function in &self.functions {
            output.push_str("        rsscript_provider_api::ProviderFunctionDescriptor {\n");
            output.push_str(&format!(
                "            symbol: rsscript_abi_model::ExternalSymbol::new({:?}).unwrap(),\n            signature: {},\n            entry: {:?}.into(),\n",
                function.symbol.as_str(),
                render_signature(&function.signature),
                function.entry
            ));
            output.push_str(&format!(
                "            call_mode: rsscript_provider_api::ProviderCallMode::{},\n            blocking: rsscript_provider_api::BlockingBehavior::{:?},\n            cancellation: rsscript_provider_api::CancellationBehavior::{:?},\n            thread_safe: {},\n            reentrant: {},\n            resource_cleanup: rsscript_provider_api::ResourceCleanupContract::{:?},\n            error_mapping: rsscript_provider_api::ProviderErrorMapping::StructuredV1,\n",
                if function.signature.asynchronous { "Async" } else { "Sync" },
                options.blocking,
                options.cancellation,
                options.thread_safe,
                options.reentrant,
                options.cleanup,
            ));
            output.push_str("        },\n");
        }
        output.push_str("    ] }\n}\n");
        output
    }
}

fn render_signature(signature: &FunctionSignature) -> String {
    let parameters = signature
        .parameters
        .iter()
        .map(|parameter| {
            format!(
                "rsscript_abi_model::ParameterSignature {{ name: {:?}.into(), effect: rsscript_abi_model::DataEffect::{:?}, ty: {:?}.into(), retained: {} }}",
                parameter.name,
                parameter.effect,
                render_wire_type(&parameter.ty),
                parameter.retained
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "rsscript_abi_model::FunctionSignature {{ parameters: vec![{parameters}], result: {:?}.into(), asynchronous: {} }}",
        render_wire_type(&signature.result),
        signature.asynchronous
    )
}

fn render_wire_type(ty: &WireType) -> String {
    match ty {
        WireType::Unit => "rsscript_abi_model::WireType::Unit".into(),
        WireType::Bool => "rsscript_abi_model::WireType::Bool".into(),
        WireType::Int { bits, signed } => {
            format!("rsscript_abi_model::WireType::Int {{ bits: {bits}, signed: {signed} }}")
        }
        WireType::Float { bits } => {
            format!("rsscript_abi_model::WireType::Float {{ bits: {bits} }}")
        }
        WireType::String => "rsscript_abi_model::WireType::String".into(),
        WireType::Bytes => "rsscript_abi_model::WireType::Bytes".into(),
        WireType::List { element } => format!(
            "rsscript_abi_model::WireType::List {{ element: Box::new({}) }}",
            render_wire_type(element)
        ),
        WireType::Option { value } => format!(
            "rsscript_abi_model::WireType::Option {{ value: Box::new({}) }}",
            render_wire_type(value)
        ),
        WireType::Result { ok, error } => format!(
            "rsscript_abi_model::WireType::Result {{ ok: Box::new({}), error: Box::new({}) }}",
            render_wire_type(ok),
            render_wire_type(error)
        ),
        WireType::Tuple { elements } => format!(
            "rsscript_abi_model::WireType::Tuple {{ elements: vec![{}] }}",
            elements
                .iter()
                .map(render_wire_type)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        WireType::Named {
            package,
            name,
            arguments,
        } => format!(
            "rsscript_abi_model::WireType::Named {{ package: {}, name: {:?}.into(), arguments: vec![{}] }}",
            package.as_ref().map_or_else(
                || "None".into(),
                |package| format!("Some({package:?}.into())")
            ),
            name,
            arguments
                .iter()
                .map(render_wire_type)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        WireType::Resource { name } => {
            format!("rsscript_abi_model::WireType::Resource {{ name: {name:?}.into() }}")
        }
        WireType::Handle { name } => {
            format!("rsscript_abi_model::WireType::Handle {{ name: {name:?}.into() }}")
        }
        WireType::Qualified { qualifier, value } => format!(
            "rsscript_abi_model::WireType::Qualified {{ qualifier: rsscript_abi_model::WireQualifier::{}, value: Box::new({}) }}",
            match qualifier {
                WireQualifier::Fresh => "Fresh",
                WireQualifier::Owned => "Owned",
                WireQualifier::NoEscape => "NoEscape",
            },
            render_wire_type(value)
        ),
    }
}

fn rust_identifier(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_is_the_signature_source_for_generated_provider_code() {
        let descriptor = InterfaceDescriptorV1::from_interface_source(
            "env.rssi",
            "module host.env\n\npub fn get(name: read String) -> Option<String>\n",
        )
        .unwrap();
        let interface = ProviderInterface::from_descriptor(descriptor).unwrap();
        assert_eq!(interface.functions[0].symbol.as_str(), "host.env.get");
        assert_eq!(interface.functions[0].signature.parameters[0].name, "name");
        let rust = interface.render_rust(&RustProviderOptions {
            provider_id: "rsscript.env",
            blocking: GeneratedBlocking::NonBlocking,
            cancellation: GeneratedCancellation::NotApplicable,
            thread_safe: true,
            reentrant: true,
            cleanup: GeneratedCleanup::None,
        });
        assert!(rust.contains("pub trait GeneratedProviderContract"));
        assert!(rust.contains("host.env.get"));
        assert!(rust.contains("WireType::Option"));
        assert!(rust.contains("WireType::String"));
    }
}
