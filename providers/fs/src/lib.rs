#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    NativeInterpreterFn, NativeValue, ProviderError, ProviderFunction, ProviderFunctionDescriptor,
};

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

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
        let mut generated = descriptor()
            .functions
            .into_iter()
            .map(|function| (function.entry.clone(), function))
            .collect::<BTreeMap<_, _>>();
        let read = generated.remove("read_text").unwrap();
        let write = generated.remove("write_text").unwrap();
        let read_provider = self.clone();
        let write_provider = self.clone();
        BTreeMap::from([
            binding(read, move |mut args| {
                let NativeValue::String(path) = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("path must be String"));
                };
                let path = read_provider
                    .resolve_existing(&path)
                    .map_err(|error| ProviderError::from_io("resolve read path", error))?;
                std::fs::read_to_string(path)
                    .map(NativeValue::String)
                    .map_err(|error| ProviderError::internal(error.to_string()))
            }),
            binding(write, move |mut args| {
                let NativeValue::String(path) = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("path must be String"));
                };
                let NativeValue::String(text) = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("text must be String"));
                };
                let path = write_provider
                    .resolve_for_write(&path)
                    .map_err(|error| ProviderError::from_io("resolve write path", error))?;
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

fn binding(
    descriptor: ProviderFunctionDescriptor,
    call: impl Fn(Vec<NativeValue>) -> Result<NativeValue, ProviderError> + Send + Sync + 'static,
) -> (ExternalSymbol, ProviderFunction<NativeInterpreterFn>) {
    (
        descriptor.symbol,
        ProviderFunction {
            signature: descriptor.signature,
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
        let mut registry =
            rsscript_provider_api::ProviderRegistry::new(rsscript_abi_model::RUNTIME_ABI_VERSION);
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
