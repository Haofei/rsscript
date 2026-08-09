use super::CliError;
use reir::Bundle;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const MAX_CLI_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CLI_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
static OUTPUT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn read_bundle(path: &str) -> Result<Bundle, CliError> {
    let json = read_bounded_text(path)?;
    Bundle::from_json(&json)
        .map_err(|error| CliError::runtime(format!("failed to parse {path}: {error}")))
}

pub(super) fn read_optional_text_accounted(
    path: Option<&str>,
    aggregate_bytes: &mut u64,
) -> Result<Option<String>, CliError> {
    path.map(|path| read_bounded_text_accounted(path, aggregate_bytes, MAX_CLI_INPUT_BYTES))
        .transpose()
}

pub(super) fn read_bounded_text_accounted(
    path: &str,
    aggregate_bytes: &mut u64,
    aggregate_limit: u64,
) -> Result<String, CliError> {
    let remaining = aggregate_limit
        .checked_sub(*aggregate_bytes)
        .ok_or_else(|| CliError::runtime("aggregate input byte limit exceeded"))?;
    let text = read_bounded_text_with_limit(Path::new(path), remaining).map_err(|error| {
        if remaining < aggregate_limit {
            let detail = match error {
                CliError::Usage(message) | CliError::Runtime(message) => message,
            };
            CliError::runtime(format!(
                "aggregate input exceeds the {aggregate_limit} byte limit: {detail}"
            ))
        } else {
            error
        }
    })?;
    *aggregate_bytes = aggregate_bytes
        .checked_add(text.len() as u64)
        .ok_or_else(|| CliError::runtime("aggregate input byte count overflowed"))?;
    Ok(text)
}

pub(super) fn read_bounded_text(path: &str) -> Result<String, CliError> {
    read_bounded_text_with_limit(Path::new(path), MAX_CLI_INPUT_BYTES)
}

pub(super) fn read_bounded_text_with_limit(path: &Path, limit: u64) -> Result<String, CliError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CliError::runtime(format!("failed to inspect {}: {error}", path.display()))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CliError::runtime(format!(
            "refusing to read symlink input {}",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(CliError::runtime(format!(
            "input is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > limit {
        return Err(CliError::runtime(format!(
            "input {} is {} bytes, exceeding the {limit} byte limit",
            path.display(),
            metadata.len()
        )));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options.open(path).map_err(|error| {
        CliError::runtime(format!("failed to read {}: {error}", path.display()))
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        CliError::runtime(format!("failed to inspect {}: {error}", path.display()))
    })?;
    if !opened_metadata.is_file() || !same_file(&metadata, &opened_metadata) {
        return Err(CliError::runtime(format!(
            "input changed while opening or is not a regular file: {}",
            path.display()
        )));
    }

    let mut bytes =
        Vec::with_capacity(usize::try_from(opened_metadata.len().min(limit)).unwrap_or(usize::MAX));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::runtime(format!("failed to read {}: {error}", path.display()))
        })?;
    if bytes.len() as u64 > limit {
        return Err(CliError::runtime(format!(
            "input {} exceeded the {limit} byte limit while reading",
            path.display()
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        CliError::runtime(format!(
            "failed to decode {} as UTF-8: {error}",
            path.display()
        ))
    })
}

pub(super) fn write_json_file(path: &str, bundle: &Bundle) -> Result<(), CliError> {
    let json = serde_json::to_string_pretty(bundle)
        .map_err(|error| CliError::runtime(format!("failed to serialize bundle: {error}")))?;
    if json.len() >= MAX_CLI_OUTPUT_BYTES {
        return Err(CliError::runtime(format!(
            "serialized bundle is too large to write within the {MAX_CLI_OUTPUT_BYTES} byte limit"
        )));
    }
    let mut output = json.into_bytes();
    output.push(b'\n');
    atomic_write_no_follow(Path::new(path), &output)
}

pub(super) fn write_bounded_text_file(path: &str, output: &str) -> Result<(), CliError> {
    if output.len() > MAX_CLI_OUTPUT_BYTES {
        return Err(CliError::runtime(format!(
            "output is too large to write within the {MAX_CLI_OUTPUT_BYTES} byte limit"
        )));
    }
    atomic_write_no_follow(Path::new(path), output.as_bytes())
}

pub(super) fn atomic_write_no_follow(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    reject_symlink_output(path)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        CliError::runtime(format!("output path has no file name: {}", path.display()))
    })?;
    let mut temp_path = None;
    let mut temp_file = None;
    for _ in 0..128 {
        let sequence = OUTPUT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{}.reir-tmp-{}-{sequence}",
            file_name.to_string_lossy(),
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        set_no_follow(&mut options);
        match options.open(&candidate) {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CliError::runtime(format!(
                    "failed to stage output {}: {error}",
                    path.display()
                )));
            }
        }
    }
    let temp_path = temp_path.ok_or_else(|| {
        CliError::runtime(format!(
            "failed to allocate a staging file for {}",
            path.display()
        ))
    })?;
    let mut temp_file = temp_file.expect("staging path and file are set together");
    let write_result = (|| {
        temp_file.write_all(bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);
        reject_symlink_output(path).map_err(|error| match error {
            CliError::Usage(message) | CliError::Runtime(message) => std::io::Error::other(message),
        })?;
        fs::rename(&temp_path, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(CliError::runtime(format!(
            "failed to atomically write {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn reject_symlink_output(path: &Path) -> Result<(), CliError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CliError::runtime(format!(
            "refusing to replace symlink output {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::runtime(format!(
            "failed to inspect output {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
pub(super) fn set_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const O_NOFOLLOW: i32 = 0x100;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    const O_NOFOLLOW: i32 = 0x20000;
    options.custom_flags(O_NOFOLLOW);
}

#[cfg(not(unix))]
pub(super) fn set_no_follow(_options: &mut OpenOptions) {}

#[cfg(unix)]
pub(super) fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
pub(super) fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
}

pub(super) fn print_json<T: serde::Serialize>(value: &T) -> Result<(), CliError> {
    let output = bounded_json(value, MAX_CLI_OUTPUT_BYTES)?;
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|error| CliError::runtime(format!("failed to write JSON to stdout: {error}")))
}

struct BoundedOutput {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let new_len = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| std::io::Error::other("JSON output byte limit exceeded"))?;
        self.bytes.reserve(new_len - self.bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn bounded_json<T: serde::Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, CliError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer_pretty(&mut output, value)
        .map_err(|error| CliError::runtime(format!("failed to serialize JSON: {error}")))?;
    output.write_all(b"\n").map_err(|error| {
        CliError::runtime(format!(
            "JSON output exceeds the {limit} byte limit: {error}"
        ))
    })?;
    Ok(output.bytes)
}
