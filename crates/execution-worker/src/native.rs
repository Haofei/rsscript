use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use rss_worker_protocol::{NativeCallRequest, NativeValue, WorkerErrorCode};
use sha2::{Digest, Sha256};

use crate::DispatchError;
use crate::conversion::{abi_to_wire, wire_to_abi};

const MAX_LIBRARY_BYTES: u64 = 1024 * 1024 * 1024;

pub(crate) fn execute(request: NativeCallRequest) -> Result<NativeValue, DispatchError> {
    let library_path = resolve_staged_path(&request.library.relative_path)?;
    verify_digest(&library_path, &request.library.sha256)?;
    let registry = rss_native_abi::load_registry(&library_path).map_err(native_error)?;
    let callable = registry
        .into_iter()
        .find_map(|(name, callable)| (name == request.binding).then_some(callable))
        .ok_or_else(|| {
            native_error(format!(
                "native registry does not contain exact binding '{}'",
                request.binding
            ))
        })?;
    let args = request.args.into_iter().map(wire_to_abi).collect();
    callable.call(args).map(abi_to_wire).map_err(native_error)
}

fn resolve_staged_path(relative_path: &str) -> Result<PathBuf, DispatchError> {
    let cwd = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(|error| native_error(format!("failed to resolve worker cwd: {error}")))?;
    let candidate = cwd.join(relative_path).canonicalize().map_err(|error| {
        native_error(format!("failed to resolve staged native library: {error}"))
    })?;
    if !candidate.starts_with(&cwd) || candidate == cwd {
        return Err(DispatchError::new(
            WorkerErrorCode::PolicyDenied,
            "native library path escapes the staged worker cwd",
        ));
    }
    let metadata = candidate
        .metadata()
        .map_err(|error| native_error(format!("failed to inspect native library: {error}")))?;
    if !metadata.is_file() {
        return Err(native_error("native library path is not a regular file"));
    }
    if metadata.len() > MAX_LIBRARY_BYTES {
        return Err(DispatchError::new(
            WorkerErrorCode::ResourceLimit,
            format!("native library exceeds the {MAX_LIBRARY_BYTES} byte limit"),
        ));
    }
    Ok(candidate)
}

fn verify_digest(path: &Path, expected: &str) -> Result<(), DispatchError> {
    let mut file = File::open(path)
        .map_err(|error| native_error(format!("failed to open native library: {error}")))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| native_error(format!("failed to hash native library: {error}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if actual != expected {
        return Err(DispatchError::new(
            WorkerErrorCode::PolicyDenied,
            format!("native library SHA-256 mismatch: expected {expected}, got {actual}"),
        ));
    }
    Ok(())
}

fn native_error(message: impl Into<String>) -> DispatchError {
    DispatchError::new(WorkerErrorCode::Native, message)
}

#[cfg(test)]
mod tests {
    use rss_worker_protocol::{NativeArtifact, NativeCallRequest};

    use super::*;

    #[test]
    fn digest_mismatch_is_rejected_before_loading() {
        let cwd = std::env::current_dir().unwrap();
        let directory = tempfile::Builder::new()
            .prefix("rss-worker-native-")
            .tempdir_in(&cwd)
            .unwrap();
        let relative = directory
            .path()
            .strip_prefix(&cwd)
            .unwrap()
            .join("plugin.bin");
        std::fs::write(cwd.join(&relative), b"not a library").unwrap();
        let result = execute(NativeCallRequest {
            library: NativeArtifact {
                relative_path: relative.to_string_lossy().into_owned(),
                sha256: "0".repeat(64),
            },
            binding: "pkg.call".to_string(),
            args: Vec::new(),
        });

        let error = result.unwrap_err();
        assert_eq!(error.code, WorkerErrorCode::PolicyDenied);
        assert!(error.message.contains("SHA-256 mismatch"));
    }
}
