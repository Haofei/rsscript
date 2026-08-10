#![forbid(unsafe_code)]

use rsscript_abi_model::{ExternalSymbol, FunctionSignature, WireQualifier, WireType};
use rsscript_semantics::{
    InterfaceDescriptorError, InterfaceDescriptorResourceV1, InterfaceDescriptorV1,
};
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
    pub resources: Vec<InterfaceDescriptorResourceV1>,
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
            resources: descriptor.resources,
        })
    }

    pub fn render_rust(&self, options: &RustProviderOptions<'_>) -> String {
        let mut output = String::from("// @generated from .rssi; do not edit.\n");
        for resource in &self.resources {
            render_resource_wrapper(&mut output, resource);
        }
        output.push_str("pub trait GeneratedProviderContract {\n");
        for function in &self.functions {
            let parameters = function
                .signature
                .parameters
                .iter()
                .map(|parameter| {
                    format!(
                        "{}: {}",
                        rust_identifier(&parameter.name),
                        render_rust_type(&parameter.ty, &self.resources)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!(
                "    {}fn {}(&self{}{}) -> Result<{}, rsscript_provider_api::ProviderError>;\n",
                if function.signature.asynchronous {
                    "async "
                } else {
                    ""
                },
                rust_identifier(&function.entry),
                if parameters.is_empty() { "" } else { ", " },
                parameters,
                render_rust_type(&function.signature.result, &self.resources),
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
        output.push_str(
            "\n/// Register generated descriptor contracts and fail closed when an implementation is missing, undeclared, or has a mismatched signature.\npub fn register<T>(\n    registry: &mut rsscript_provider_api::ProviderRegistry<T>,\n    implementations: std::collections::BTreeMap<rsscript_abi_model::ExternalSymbol, rsscript_provider_api::ProviderFunction<T>>,\n) -> Result<(), rsscript_provider_api::ProviderLoadError> {\n    registry.register_provider(&descriptor(), implementations)\n}\n",
        );
        render_mock_support(&mut output, &self.functions);
        output
    }
}

fn render_mock_support(output: &mut String, functions: &[InterfaceFunction]) {
    output.push_str(
        "\n/// One call observed by the generated contract mock. The mock deliberately\n/// records dynamic boundary values; typed conversion remains in the generated\n/// adapter owned by the real Provider implementation.\n#[derive(Debug, Clone)]\npub struct MockCall {\n    pub symbol: rsscript_abi_model::ExternalSymbol,\n    pub args: Vec<rsscript_provider_api::NativeValue>,\n}\n\n#[derive(Clone, Default)]\npub struct MockProvider {\n    calls: std::sync::Arc<std::sync::Mutex<Vec<MockCall>>>,\n}\n\nimpl MockProvider {\n    pub fn calls(&self) -> Vec<MockCall> {\n        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()\n    }\n\n    pub fn implementations(&self) -> std::collections::BTreeMap<rsscript_abi_model::ExternalSymbol, rsscript_provider_api::ProviderFunction<rsscript_provider_api::ProviderCallable>> {\n        let mut implementations = std::collections::BTreeMap::new();\n",
    );
    for function in functions {
        let symbol = format!(
            "rsscript_abi_model::ExternalSymbol::new({:?}).expect(\"generated symbol is valid\")",
            function.symbol.as_str()
        );
        let signature = render_signature(&function.signature);
        let callable = if function.signature.asynchronous {
            format!(
                "rsscript_provider_api::ProviderCallable::Async(rsscript_provider_api::AsyncInterpreterFn::new({{ let calls = std::sync::Arc::clone(&self.calls); move |_context, args| {{ let calls = std::sync::Arc::clone(&calls); async move {{ calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(MockCall {{ symbol: {symbol}, args }}); Err(rsscript_provider_api::ProviderError::unavailable(\"generated mock has no configured response\")) }} }} }}))"
            )
        } else {
            format!(
                "rsscript_provider_api::ProviderCallable::Sync(rsscript_provider_api::NativeInterpreterFn::new_contextual({{ let calls = std::sync::Arc::clone(&self.calls); move |_context, args| {{ calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(MockCall {{ symbol: {symbol}, args }}); Err(rsscript_provider_api::ProviderError::unavailable(\"generated mock has no configured response\")) }} }}))"
            )
        };
        output.push_str(&format!(
            "        implementations.insert({symbol}, rsscript_provider_api::ProviderFunction {{ signature: {signature}, callable: {callable} }});\n"
        ));
    }
    output.push_str(
        "        implementations\n    }\n\n    /// Register every descriptor-declared symbol through the same fail-closed\n    /// path as a real Provider. This is useful for integration tests that only\n    /// need contract completeness.\n    pub fn register(&self, registry: &mut rsscript_provider_api::ProviderRegistry<rsscript_provider_api::ProviderCallable>) -> Result<(), rsscript_provider_api::ProviderLoadError> {\n        register(registry, self.implementations())\n    }\n}\n\n/// Generated contract test skeleton. Provider crates can call this from a unit\n/// test before adding behavior-specific mock responses.\n#[cfg(test)]\npub fn assert_generated_mock_contract() {\n    let mock = MockProvider::default();\n    let mut registry = rsscript_provider_api::ProviderRegistry::new(rsscript_abi_model::RUNTIME_ABI_VERSION);\n    mock.register(&mut registry).expect(\"generated mock must cover every descriptor symbol\");\n}\n",
    );
}

fn render_resource_wrapper(output: &mut String, resource: &InterfaceDescriptorResourceV1) {
    let type_name = resource_wrapper_name(&resource.name);
    output.push_str(&format!(
        "/// Generated handle wrapper for the `{}` resource.\n#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub struct {type_name}(pub rsscript_provider_api::ResourceHandle);\n\nimpl {type_name} {{\n    /// Legacy name used only by the `NativeValue` compatibility adapter.\n    pub const TYPE_NAME: &'static str = {:?};\n\n    /// Decode the canonical, generation-safe Provider wire representation.\n    /// `resource_type` comes from the linked interface descriptor; it is never\n    /// inferred from a legacy type-name string.\n    pub fn from_wire(\n        value: rsscript_abi_model::WireValue,\n        resource_type: rsscript_abi_model::WireResourceTypeId,\n    ) -> Result<Self, rsscript_provider_api::ProviderError> {{\n        match value {{\n            rsscript_abi_model::WireValue::Resource {{ handle }} if handle.resource_type == resource_type => Ok(Self(rsscript_provider_api::ResourceHandle::from_wire(handle))),\n            _ => Err(rsscript_provider_api::ProviderError::invalid_argument(\"resource handle type mismatch\")),\n        }}\n    }}\n\n    /// Encode the canonical, generation-safe Provider wire representation.\n    pub fn into_wire(\n        self,\n        resource_type: rsscript_abi_model::WireResourceTypeId,\n    ) -> rsscript_abi_model::WireValue {{\n        rsscript_abi_model::WireValue::Resource {{ handle: self.0.to_wire(resource_type) }}\n    }}\n\n    /// Decode the legacy dynamic adapter representation. New Provider code\n    /// should prefer `from_wire`.\n    pub fn from_native(value: rsscript_provider_api::NativeValue) -> Result<Self, rsscript_provider_api::ProviderError> {{\n        match value {{\n            rsscript_provider_api::NativeValue::Native {{ type_name, id }} if type_name == Self::TYPE_NAME => Ok(Self(rsscript_provider_api::ResourceHandle::from_native_id(id))),\n            _ => Err(rsscript_provider_api::ProviderError::invalid_argument(\"resource handle type mismatch\")),\n        }}\n    }}\n\n    /// Encode the legacy dynamic adapter representation. New Provider code\n    /// should prefer `into_wire`.\n    pub fn into_native(self) -> rsscript_provider_api::NativeValue {{\n        rsscript_provider_api::NativeValue::Native {{ type_name: Self::TYPE_NAME.into(), id: self.0.to_native_id() }}\n    }}\n}}\n\n",
        resource.name,
        resource.name,
    ));
}

fn resource_wrapper_name(resource: &str) -> String {
    let mut output = resource
        .split('.')
        .flat_map(str::chars)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty()
        || output
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        output.insert(0, '_');
    }
    output.push_str("Handle");
    output
}

fn render_rust_type(ty: &WireType, resources: &[InterfaceDescriptorResourceV1]) -> String {
    match ty {
        WireType::Unit => "()".into(),
        WireType::Bool => "bool".into(),
        WireType::Int { .. } => "i64".into(),
        WireType::Float { .. } => "f64".into(),
        WireType::String => "String".into(),
        WireType::Bytes => "Vec<u8>".into(),
        WireType::List { element } => format!("Vec<{}>", render_rust_type(element, resources)),
        WireType::Option { value } => format!("Option<{}>", render_rust_type(value, resources)),
        WireType::Result { ok, error } => format!(
            "Result<{}, {}>",
            render_rust_type(ok, resources),
            render_rust_type(error, resources)
        ),
        WireType::Tuple { elements } => format!(
            "({})",
            elements
                .iter()
                .map(|element| render_rust_type(element, resources))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        WireType::Qualified { value, .. } => render_rust_type(value, resources),
        WireType::Named { name, .. } | WireType::Resource { name } | WireType::Handle { name } => {
            resource_wrapper_for(name, resources)
                .unwrap_or_else(|| "rsscript_provider_api::NativeValue".into())
        }
    }
}

fn resource_wrapper_for(
    type_name: &str,
    resources: &[InterfaceDescriptorResourceV1],
) -> Option<String> {
    resources
        .iter()
        .find(|resource| {
            resource.name == type_name
                || resource
                    .name
                    .rsplit_once('.')
                    .is_some_and(|(_, local)| local == type_name)
        })
        .map(|resource| resource_wrapper_name(&resource.name))
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
                wire_type_source(&parameter.ty),
                parameter.retained
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "rsscript_abi_model::FunctionSignature {{ parameters: vec![{parameters}], result: {:?}.into(), asynchronous: {} }}",
        wire_type_source(&signature.result),
        signature.asynchronous
    )
}

/// Render the canonical RSScript type spelling carried by the legacy
/// `FunctionSignature` compatibility envelope. This must not use
/// `render_wire_type`: that helper emits Rust source expressions for generated
/// descriptor code, not the language ABI text that Artifact imports compare.
fn wire_type_source(ty: &WireType) -> String {
    match ty {
        WireType::Unit => "Unit".into(),
        WireType::Bool => "Bool".into(),
        WireType::Int { .. } => "Int".into(),
        WireType::Float { .. } => "Float".into(),
        WireType::String => "String".into(),
        WireType::Bytes => "Bytes".into(),
        WireType::List { element } => format!("List<{}>", wire_type_source(element)),
        WireType::Option { value } => format!("Option<{}>", wire_type_source(value)),
        WireType::Result { ok, error } => format!(
            "Result<{}, {}>",
            wire_type_source(ok),
            wire_type_source(error)
        ),
        WireType::Tuple { elements } => format!(
            "({})",
            elements
                .iter()
                .map(wire_type_source)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        WireType::Named {
            package,
            name,
            arguments,
        } => {
            let root = package
                .as_ref()
                .map_or_else(|| name.clone(), |package| format!("{package}.{name}"));
            if arguments.is_empty() {
                root
            } else {
                format!(
                    "{root}<{}>",
                    arguments
                        .iter()
                        .map(wire_type_source)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        WireType::Resource { name } | WireType::Handle { name } => name.clone(),
        WireType::Qualified { qualifier, value } => format!(
            "{} {}",
            match qualifier {
                WireQualifier::Fresh => "fresh",
                WireQualifier::Owned => "owned",
                WireQualifier::NoEscape => "noescape",
            },
            wire_type_source(value)
        ),
    }
}

#[allow(dead_code)]
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
        assert!(rust.contains("fn get(&self, name: String) -> Result<Option<String>"));
        assert!(rust.contains("host.env.get"));
        assert!(rust.contains("ty: \"String\".into()"));
        assert!(rust.contains("result: \"Option<String>\".into()"));
        assert!(rust.contains("pub fn register<T>("));
        assert!(rust.contains("registry.register_provider(&descriptor(), implementations)"));
        assert!(rust.contains("pub struct MockProvider"));
        assert!(rust.contains("pub fn implementations(&self)"));
        assert!(rust.contains("assert_generated_mock_contract"));
    }

    #[test]
    fn generated_resources_use_typed_generation_safe_handle_wrappers() {
        let descriptor = InterfaceDescriptorV1::from_interface_source(
            "fs.rssi",
            "module host.fs\n\npub resource File\npub fn open(path: read String) -> File\n",
        )
        .unwrap();
        let interface = ProviderInterface::from_descriptor(descriptor).unwrap();
        assert_eq!(interface.resources[0].name, "host.fs.File");
        let rust = interface.render_rust(&RustProviderOptions {
            provider_id: "rsscript.fs",
            blocking: GeneratedBlocking::MayBlock,
            cancellation: GeneratedCancellation::Cooperative,
            thread_safe: true,
            reentrant: false,
            cleanup: GeneratedCleanup::RuntimeRegistered,
        });
        assert!(
            rust.contains("pub struct hostfsFileHandle(pub rsscript_provider_api::ResourceHandle)")
        );
        assert!(rust.contains("ResourceHandle::from_native_id"));
        assert!(rust.contains("pub fn from_wire("));
        assert!(rust.contains("rsscript_abi_model::WireValue::Resource"));
        assert!(rust.contains("handle.resource_type == resource_type"));
        assert!(rust.contains("ResourceHandle::from_wire(handle)"));
        assert!(rust.contains("pub fn into_wire("));
        assert!(rust.contains("self.0.to_wire(resource_type)"));
        assert!(rust.contains("pub const TYPE_NAME: &'static str = \"host.fs.File\""));
        assert!(rust.contains("fn open(&self, path: String) -> Result<hostfsFileHandle"));
    }

    #[test]
    fn generated_descriptor_uses_language_type_spelling_not_rust_expressions() {
        let descriptor = InterfaceDescriptorV1::from_interface_source(
            "test.rssi",
            "module host.test\npub fn run(value: read String) -> Option<Int>\n",
        )
        .unwrap();
        let generated = ProviderInterface::from_descriptor(descriptor)
            .unwrap()
            .render_rust(&RustProviderOptions {
                provider_id: "rsscript.test",
                blocking: GeneratedBlocking::NonBlocking,
                cancellation: GeneratedCancellation::NotApplicable,
                thread_safe: true,
                reentrant: true,
                cleanup: GeneratedCleanup::None,
            });
        assert!(generated.contains("ty: \"String\".into()"));
        assert!(generated.contains("result: \"Option<Int>\".into()"));
        assert!(!generated.contains("ty: \"rsscript_abi_model::WireType"));
    }

    #[test]
    fn generated_async_methods_and_descriptor_call_modes_share_one_contract() {
        let descriptor = InterfaceDescriptorV1::from_interface_source(
            "log.rssi",
            "module host.log\n\npub async fn emit(value: take String) -> Unit retains(value)\n",
        )
        .unwrap();
        let interface = ProviderInterface::from_descriptor(descriptor).unwrap();
        let rust = interface.render_rust(&RustProviderOptions {
            provider_id: "rsscript.log",
            blocking: GeneratedBlocking::NonBlocking,
            cancellation: GeneratedCancellation::Cooperative,
            thread_safe: true,
            reentrant: true,
            cleanup: GeneratedCleanup::None,
        });
        assert!(rust.contains("async fn emit(&self, value: String) -> Result<(),"));
        assert!(rust.contains("ProviderCallMode::Async"));
        assert!(rust.contains("DataEffect::Take"));
        assert!(rust.contains("retained: true"));
        assert!(rust.contains("ProviderCallable::Async"));
    }
}
