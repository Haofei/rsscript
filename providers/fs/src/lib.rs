#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, RUNTIME_ABI_VERSION,
};
use rsscript_provider_api::{
    BlockingBehavior, CancellationBehavior, NativeInterpreterFn, NativeValue, ProviderCallMode,
    ProviderDescriptor, ProviderError, ProviderFunction, ProviderFunctionDescriptor,
};

pub fn descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        provider_id: "rsscript.fs".into(),
        provider_version: env!("CARGO_PKG_VERSION").into(),
        supported_abi: vec![RUNTIME_ABI_VERSION],
        functions: vec![
            function(
                "host.fs.read_text",
                signature(vec![("path", DataEffect::Read, "String")], "String"),
                "read_text",
            ),
            function(
                "host.fs.write_text",
                signature(
                    vec![
                        ("path", DataEffect::Read, "String"),
                        ("text", DataEffect::Read, "String"),
                    ],
                    "Unit",
                ),
                "write_text",
            ),
        ],
    }
}

/// Filesystem authority rooted at one host-selected directory.
///
/// Script paths must be relative and cannot escape through `..`, absolute
/// paths, or symlinks. The authority is carried by this Provider instance; no
/// process-global current-directory mutation is required.
#[derive(Debug, Clone)]
pub struct RootedFsProvider {
    root: PathBuf,
}

impl RootedFsProvider {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "filesystem provider root must be a directory",
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn functions(&self) -> BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>> {
        let read = signature(vec![("path", DataEffect::Read, "String")], "String");
        let write = signature(
            vec![
                ("path", DataEffect::Read, "String"),
                ("text", DataEffect::Read, "String"),
            ],
            "Unit",
        );
        let read_provider = self.clone();
        let write_provider = self.clone();
        BTreeMap::from([
            binding("host.fs.read_text", read, move |mut args| {
                let NativeValue::String(path) = args.remove(0) else {
                    return Err("path must be String".into());
                };
                let path = read_provider
                    .resolve_existing(&path)
                    .map_err(|error| error.to_string())?;
                std::fs::read_to_string(path)
                    .map(NativeValue::String)
                    .map_err(|error| ProviderError::internal(error.to_string()))
            }),
            binding("host.fs.write_text", write, move |mut args| {
                let NativeValue::String(path) = args.remove(0) else {
                    return Err("path must be String".into());
                };
                let NativeValue::String(text) = args.remove(0) else {
                    return Err("text must be String".into());
                };
                let path = write_provider
                    .resolve_for_write(&path)
                    .map_err(|error| error.to_string())?;
                std::fs::write(path, text)
                    .map(|_| NativeValue::Unit)
                    .map_err(|error| ProviderError::internal(error.to_string()))
            }),
        ])
    }

    fn relative_path<'a>(&self, path: &'a str) -> io::Result<&'a Path> {
        let path = Path::new(path);
        if path.as_os_str().is_empty()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "filesystem provider path must stay below its configured root",
            ));
        }
        Ok(path)
    }

    fn resolve_existing(&self, path: &str) -> io::Result<PathBuf> {
        let candidate = std::fs::canonicalize(self.root.join(self.relative_path(path)?))?;
        self.ensure_below_root(candidate)
    }

    fn resolve_for_write(&self, path: &str) -> io::Result<PathBuf> {
        let relative = self.relative_path(path)?;
        let candidate = self.root.join(relative);
        if candidate.exists() {
            return self.ensure_below_root(std::fs::canonicalize(candidate)?);
        }
        let parent = candidate.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "write path has no parent")
        })?;
        let canonical_parent = self.ensure_below_root(std::fs::canonicalize(parent)?)?;
        let file_name = candidate.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "write path has no file name")
        })?;
        Ok(canonical_parent.join(file_name))
    }

    fn ensure_below_root(&self, path: PathBuf) -> io::Result<PathBuf> {
        if path.starts_with(&self.root) {
            Ok(path)
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "filesystem provider path escapes its configured root",
            ))
        }
    }
}

fn signature(params: Vec<(&str, DataEffect, &str)>, result: &str) -> FunctionSignature {
    FunctionSignature {
        parameters: params
            .into_iter()
            .map(|(name, effect, type_name)| ParameterSignature {
                name: name.into(),
                effect,
                ty: type_name.into(),
                retained: false,
            })
            .collect(),
        result: result.into(),
        asynchronous: false,
    }
}
fn function(symbol: &str, signature: FunctionSignature, entry: &str) -> ProviderFunctionDescriptor {
    ProviderFunctionDescriptor {
        symbol: ExternalSymbol::new(symbol).unwrap(),
        signature,
        entry: entry.into(),
        call_mode: ProviderCallMode::Sync,
        blocking: BlockingBehavior::MayBlock,
        cancellation: CancellationBehavior::NotApplicable,
        thread_safe: true,
        reentrant: true,
        resource_cleanup: rsscript_provider_api::ResourceCleanupContract::None,
        error_mapping: rsscript_provider_api::ProviderErrorMapping::StructuredV1,
    }
}
fn binding(
    symbol: &str,
    signature: FunctionSignature,
    call: impl Fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError> + Send + Sync + 'static,
) -> (ExternalSymbol, ProviderFunction<NativeInterpreterFn>) {
    (
        ExternalSymbol::new(symbol).unwrap(),
        ProviderFunction {
            signature,
            callable: NativeInterpreterFn::new(call),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_and_implementations_link() {
        let root = std::env::temp_dir().join(format!(
            "rsscript-provider-fs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("link")
        ));
        std::fs::create_dir_all(&root).unwrap();
        let provider = RootedFsProvider::new(&root).unwrap();
        let mut registry = rsscript_provider_api::ProviderRegistry::new(RUNTIME_ABI_VERSION);
        registry
            .register_provider(&descriptor(), provider.functions())
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rooted_provider_rejects_parent_paths() {
        let root = std::env::temp_dir().join(format!(
            "rsscript-provider-fs-boundary-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let provider = RootedFsProvider::new(&root).unwrap();
        let error = provider.resolve_for_write("../outside.txt").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_dir_all(root).unwrap();
    }
}
