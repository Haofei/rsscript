#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::io::Read;
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;

use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    ProviderCallContext, ProviderError, ProviderFunction, ProviderFunctionDescriptor,
    WireInterpreterFn, WireValue,
};

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

const MAX_READ_BYTES: usize = 16 * 1024 * 1024;

/// Filesystem authority rooted at one host-selected directory.
///
/// Script paths must be relative and cannot escape through `..`, absolute
/// paths, or symlinks. The authority is carried by this Provider instance; no
/// process-global current-directory mutation is required.
#[derive(Debug, Clone)]
pub struct RootedFsProvider {
    root: PathBuf,
    #[cfg(unix)]
    root_descriptor: Arc<std::os::fd::OwnedFd>,
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
        #[cfg(unix)]
        {
            let root_descriptor = Arc::new(
                rustix::fs::open(
                    &root,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(io::Error::from)?,
            );
            Ok(Self {
                root,
                root_descriptor,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "race-resistant rooted filesystem access is not implemented on this platform",
            ))
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical wire implementation. The filesystem interface is scalar-only;
    /// filesystem authority stays instance-owned by this Provider.
    pub fn functions(&self) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
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
            binding(read, move |context, mut args| {
                context.check_cancelled()?;
                let WireValue::String { value: path } = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("path must be String"));
                };
                let file = read_provider
                    .open_for_read(&path)
                    .map_err(|error| ProviderError::from_io("open read path", error))?;
                let limit = context
                    .remaining_byte_budget
                    .map_or(MAX_READ_BYTES, |budget| budget.min(MAX_READ_BYTES));
                let metadata = file
                    .metadata()
                    .map_err(|error| ProviderError::from_io("inspect read path", error))?;
                if metadata.len() > limit as u64 {
                    return Err(ProviderError::resource_exhausted(format!(
                        "filesystem read exceeds {limit} bytes"
                    )));
                }
                let mut text = String::with_capacity(metadata.len() as usize);
                file.take(limit as u64 + 1)
                    .read_to_string(&mut text)
                    .map_err(|error| ProviderError::from_io("read text", error))?;
                if text.len() > limit {
                    return Err(ProviderError::resource_exhausted(format!(
                        "filesystem read exceeds {limit} bytes"
                    )));
                }
                context.check_cancelled()?;
                Ok(WireValue::String { value: text })
            }),
            binding(write, move |context, mut args| {
                context.check_cancelled()?;
                let WireValue::String { value: path } = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("path must be String"));
                };
                let WireValue::String { value: text } = args.remove(0) else {
                    return Err(ProviderError::invalid_argument("text must be String"));
                };
                write_provider
                    .write_text(&path, text.as_bytes())
                    .map_err(|error| ProviderError::from_io("write text", error))?;
                Ok(WireValue::Unit)
            }),
        ])
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
    fn open_for_read(&self, path: &str) -> io::Result<std::fs::File> {
        self.open_relative(
            self.relative_path(path)?,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
    }

    #[cfg(not(unix))]
    fn open_for_read(&self, _path: &str) -> io::Result<std::fs::File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant rooted filesystem access is not implemented on this platform",
        ))
    }

    #[cfg(unix)]
    fn write_text(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut file = self.open_relative(
            self.relative_path(path)?,
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::TRUNC
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    #[cfg(not(unix))]
    fn write_text(&self, _path: &str, _bytes: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant rooted filesystem access is not implemented on this platform",
        ))
    }

    /// Traverse every component relative to a stable directory descriptor.
    /// `NOFOLLOW` is applied at every hop and the final open is relative to
    /// the already-open parent, closing the canonicalize/open race.
    #[cfg(unix)]
    fn open_relative(
        &self,
        relative: &Path,
        final_flags: rustix::fs::OFlags,
        final_mode: rustix::fs::Mode,
    ) -> io::Result<std::fs::File> {
        use rustix::fs::{Mode, OFlags};
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(component) => Ok(component),
                _ => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "filesystem provider path must stay below its configured root",
                )),
            })
            .collect::<io::Result<Vec<_>>>()?;
        let (name, parents) = components.split_last().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "filesystem path is empty")
        })?;
        let mut current = rustix::io::dup(&*self.root_descriptor).map_err(io::Error::from)?;
        for component in parents {
            current = rustix::fs::openat(
                &current,
                *component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
        }
        let file = rustix::fs::openat(&current, *name, final_flags | OFlags::NOFOLLOW, final_mode)
            .map_err(io::Error::from)?;
        Ok(file.into())
    }
}

fn binding(
    descriptor: ProviderFunctionDescriptor,
    call: impl for<'a> Fn(
        &mut ProviderCallContext<'a>,
        Vec<WireValue>,
    ) -> Result<WireValue, ProviderError>
    + Send
    + Sync
    + 'static,
) -> (ExternalSymbol, ProviderFunction<WireInterpreterFn>) {
    (
        descriptor.symbol,
        ProviderFunction {
            signature: descriptor.signature,
            callable: WireInterpreterFn::new_contextual(call),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conforms_to_provider_contract() {
        let root = std::env::temp_dir().join(format!(
            "rsscript-provider-fs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("link")
        ));
        std::fs::create_dir_all(&root).unwrap();
        let provider = RootedFsProvider::new(&root).unwrap();
        let report = rsscript_provider_conformance::assert_wire_provider_conforms(
            descriptor(),
            provider.functions(),
        );
        assert_eq!(report.provider_id, "rsscript.fs");
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
        let error = provider.relative_path("../outside.txt").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_obeys_runtime_byte_budget() {
        let root = std::env::temp_dir().join(format!(
            "rsscript-provider-fs-budget-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("large.txt"), "0123456789abcdef").unwrap();
        let provider = RootedFsProvider::new(&root).unwrap();
        let read = provider
            .functions()
            .into_iter()
            .find(|(symbol, _)| symbol.as_str() == "host.fs.read_text")
            .unwrap()
            .1;
        let mut context = ProviderCallContext {
            remaining_byte_budget: Some(8),
            blocking_allowed: true,
            ..ProviderCallContext::default()
        };
        let error = read
            .callable
            .call_with_context(
                &mut context,
                vec![WireValue::String {
                    value: "large.txt".into(),
                }],
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::ResourceExhausted
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rooted_provider_rejects_symlink_at_final_open() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "rsscript-provider-fs-symlink-{}",
            std::process::id()
        ));
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(base.join("outside.txt"), "secret").unwrap();
        symlink(base.join("outside.txt"), root.join("escape.txt")).unwrap();
        let provider = RootedFsProvider::new(&root).unwrap();
        provider.open_for_read("escape.txt").unwrap_err();
        std::fs::remove_dir_all(base).unwrap();
    }
}
