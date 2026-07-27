use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::ResourceBudget;
use crate::async_runtime::{NativeAsyncPending, spawn_tokio_native};
use crate::channel::{ChannelError, RssStream, stream_from_iterator};
use crate::diagnostics::Resource;
use tokio::io::AsyncReadExt;

pub const RUNTIME_READ_CEILING_BYTES: usize = 64 * 1024 * 1024;
pub const RUNTIME_DIRECTORY_MAX_DEPTH: usize = 64;
pub const RUNTIME_DIRECTORY_MAX_ENTRIES: usize = 100_000;
pub const RUNTIME_DIRECTORY_MAX_PATH_BYTES: usize = 16 * 1024 * 1024;

pub struct File {
    pub(crate) inner: std::fs::File,
}

impl Resource for File {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError {
    kind: String,
    message: String,
}

impl FileError {
    pub fn new(kind: &str, message: &str) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.to_string(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FileError {}

impl From<std::io::Error> for FileError {
    fn from(error: std::io::Error) -> Self {
        Self {
            kind: format!("{:?}", error.kind()),
            message: error.to_string(),
        }
    }
}

pub trait RuntimePath {
    fn as_path(&self) -> &std::path::Path;
}

impl RuntimePath for std::path::Path {
    fn as_path(&self) -> &std::path::Path {
        self
    }
}

impl RuntimePath for PathBuf {
    fn as_path(&self) -> &std::path::Path {
        self.as_path()
    }
}

impl RuntimePath for String {
    fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(self)
    }
}

impl RuntimePath for str {
    fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(self)
    }
}

impl<T: RuntimePath + ?Sized> RuntimePath for &T {
    fn as_path(&self) -> &std::path::Path {
        (*self).as_path()
    }
}

pub fn path_from_string(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn path_join<P: RuntimePath + ?Sized>(base: &P, child: &str) -> PathBuf {
    base.as_path().join(child)
}

pub fn path_safe_relative(value: &str) -> Result<PathBuf, String> {
    let path = std::path::Path::new(value);
    if value.is_empty() {
        return Err("path must be non-empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                return Err("parent-directory traversal is not allowed".to_string());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("path must name a relative file or directory".to_string());
    }
    Ok(normalized)
}

pub fn path_resolve_relative<P: RuntimePath + ?Sized>(
    root: &P,
    relative: &str,
) -> Result<PathBuf, String> {
    let relative = path_safe_relative(relative)?;
    let root = root.as_path();
    let resolved = path_normalize(&root.join(relative));
    if !resolved.starts_with(root) {
        return Err("resolved path escapes the workspace root".to_string());
    }
    Ok(resolved)
}

pub fn path_normalize<P: RuntimePath + ?Sized>(path: &P) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.as_path().components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub fn path_to_string<P: RuntimePath + ?Sized>(path: &P) -> String {
    path.as_path().to_string_lossy().to_string()
}

pub fn path_file_name<P: RuntimePath + ?Sized>(path: &P) -> Option<String> {
    path.as_path()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

pub fn path_extension<P: RuntimePath + ?Sized>(path: &P) -> Option<String> {
    path.as_path()
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
}

pub fn path_parent<P: RuntimePath + ?Sized>(path: &P) -> Option<PathBuf> {
    path.as_path().parent().map(PathBuf::from)
}

pub fn path_is_absolute<P: RuntimePath + ?Sized>(path: &P) -> bool {
    path.as_path().is_absolute()
}

pub fn path_with_extension<P: RuntimePath + ?Sized>(path: &P, extension: &str) -> PathBuf {
    let mut path = path.as_path().to_path_buf();
    path.set_extension(extension);
    path
}

pub fn path_starts_with<P: RuntimePath + ?Sized, Q: RuntimePath + ?Sized>(
    path: &P,
    base: &Q,
) -> bool {
    path.as_path().starts_with(base.as_path())
}

pub trait RuntimeBytes {
    fn as_bytes_slice(&self) -> &[u8];
}

impl RuntimeBytes for Vec<u8> {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_slice()
    }
}

impl RuntimeBytes for [u8] {
    fn as_bytes_slice(&self) -> &[u8] {
        self
    }
}

impl RuntimeBytes for String {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl RuntimeBytes for str {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<T: RuntimeBytes + ?Sized> RuntimeBytes for &T {
    fn as_bytes_slice(&self) -> &[u8] {
        (*self).as_bytes_slice()
    }
}

pub fn file_error_message(error: &FileError) -> String {
    error.message().to_string()
}

pub fn file_open<P: RuntimePath + ?Sized>(path: &P) -> Result<File, FileError> {
    file_open_read(path)
}

pub fn file_open_read<P: RuntimePath + ?Sized>(path: &P) -> Result<File, FileError> {
    std::fs::File::open(path.as_path())
        .map(|inner| File { inner })
        .map_err(FileError::from)
}

pub fn file_open_write<P: RuntimePath + ?Sized>(path: &P) -> Result<File, FileError> {
    std::fs::File::create(path.as_path())
        .map(|inner| File { inner })
        .map_err(FileError::from)
}

pub fn file_exists<P: RuntimePath + ?Sized>(path: &P) -> bool {
    path.as_path().exists()
}

pub fn file_read_bytes<P: RuntimePath + ?Sized>(path: &P) -> Result<Vec<u8>, FileError> {
    read_path_bounded(path.as_path())
}

pub fn file_read_bytes_with_budget<P: RuntimePath + ?Sized>(
    path: &P,
    budget: &ResourceBudget,
) -> Result<Vec<u8>, FileError> {
    let mut file = std::fs::File::open(path.as_path())?;
    read_file_remaining_with_budget(&mut file, budget)
}

pub fn file_read_bytes_from_offset<P: RuntimePath + ?Sized>(
    path: &P,
    offset: u64,
) -> Result<Vec<u8>, FileError> {
    let mut file = std::fs::File::open(path.as_path())?;
    file.seek(SeekFrom::Start(offset))?;
    read_file_remaining_bounded(&mut file)
}

pub fn file_read_bytes_from_offset_with_budget<P: RuntimePath + ?Sized>(
    path: &P,
    offset: u64,
    budget: &ResourceBudget,
) -> Result<Vec<u8>, FileError> {
    let mut file = std::fs::File::open(path.as_path())?;
    file.seek(SeekFrom::Start(offset))?;
    read_file_remaining_with_budget(&mut file, budget)
}

pub fn file_read_string<P: RuntimePath + ?Sized>(path: &P) -> Result<String, FileError> {
    bytes_to_string(read_path_bounded(path.as_path())?)
}

pub fn file_read_string_with_budget<P: RuntimePath + ?Sized>(
    path: &P,
    budget: &ResourceBudget,
) -> Result<String, FileError> {
    bytes_to_string(file_read_bytes_with_budget(path, budget)?)
}

pub fn file_write_bytes<P: RuntimePath + ?Sized, B: RuntimeBytes + ?Sized>(
    path: &P,
    data: &B,
) -> Result<(), FileError> {
    std::fs::write(path.as_path(), data.as_bytes_slice()).map_err(FileError::from)
}

pub fn file_append_bytes<P: RuntimePath + ?Sized, B: RuntimeBytes + ?Sized>(
    path: &P,
    data: &B,
) -> Result<(), FileError> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_path())?;
    file.write_all(data.as_bytes_slice())
        .map_err(FileError::from)
}

pub fn file_write_string_to_path<P: RuntimePath + ?Sized>(
    path: &P,
    text: &str,
) -> Result<(), FileError> {
    std::fs::write(path.as_path(), text.as_bytes()).map_err(FileError::from)
}

pub fn file_write_atomic<P: RuntimePath + ?Sized>(path: &P, text: &str) -> Result<(), FileError> {
    let path = path.as_path();
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "rsscript-atomic-write".to_string());
    let temp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    match std::fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
}

pub fn file_append_string<P: RuntimePath + ?Sized>(path: &P, text: &str) -> Result<(), FileError> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_path())?;
    file.write_all(text.as_bytes()).map_err(FileError::from)
}

pub fn file_remove<P: RuntimePath + ?Sized>(path: &P) -> Result<(), FileError> {
    std::fs::remove_file(path.as_path()).map_err(FileError::from)
}

pub fn file_read_all(file: &mut File) -> Result<Vec<u8>, FileError> {
    file_read_all_with_budget(
        file,
        &ResourceBudget::new(RUNTIME_READ_CEILING_BYTES as u64),
    )
}

pub fn file_read_all_with_budget(
    file: &mut File,
    budget: &ResourceBudget,
) -> Result<Vec<u8>, FileError> {
    let original_position = file.inner.stream_position()?;
    match read_file_remaining_with_budget(&mut file.inner, budget) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            let _ = file.inner.seek(SeekFrom::Start(original_position));
            Err(error)
        }
    }
}

pub fn file_read_all_string(file: &mut File) -> Result<String, FileError> {
    bytes_to_string(file_read_all(file)?)
}

pub fn file_read_into(file: &mut File, buffer: &mut Vec<u8>) -> Result<bool, FileError> {
    let budget = ResourceBudget::new(buffer.capacity() as u64);
    file_read_into_with_budget(file, buffer, &budget)
}

pub fn file_read_into_with_budget(
    file: &mut File,
    buffer: &mut Vec<u8>,
    budget: &ResourceBudget,
) -> Result<bool, FileError> {
    let limit = buffer.capacity();
    buffer.clear();
    if limit == 0 {
        return Ok(false);
    }
    let reservation = budget.try_reserve(limit).map_err(|error| {
        FileError::new(
            "ResourceBudget",
            &format!("file read byte budget exhausted: {error}"),
        )
    })?;
    let bytes_read = Read::by_ref(&mut file.inner)
        .take(limit as u64)
        .read_to_end(buffer)?;
    reservation.commit(bytes_read);
    Ok(bytes_read > 0)
}

pub fn file_bytes_stream<P: RuntimePath + ?Sized>(
    path: &P,
    chunk_size: i64,
) -> Result<RssStream<Vec<u8>>, ChannelError> {
    if chunk_size > RUNTIME_READ_CEILING_BYTES as i64 {
        return Err(ChannelError::new(&format!(
            "file byte stream chunk size {chunk_size} exceeds runtime read ceiling of \
             {RUNTIME_READ_CEILING_BYTES} bytes"
        )));
    }
    let file = std::fs::File::open(path.as_path())
        .map_err(|error| ChannelError::new(&format!("file byte stream open failed: {error}")))?;
    let chunk_size = chunk_size.max(1) as usize;
    Ok(stream_from_iterator(FileBytesIterator {
        file,
        chunk_size,
        done: false,
    }))
}

struct FileBytesIterator {
    file: std::fs::File,
    chunk_size: usize,
    done: bool,
}

impl Iterator for FileBytesIterator {
    type Item = Result<Vec<u8>, ChannelError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut buffer = vec![0; self.chunk_size];
        match self.file.read(&mut buffer) {
            Ok(0) => {
                self.done = true;
                None
            }
            Ok(bytes_read) => {
                buffer.truncate(bytes_read);
                Some(Ok(buffer))
            }
            Err(error) => {
                self.done = true;
                Some(Err(ChannelError::new(&format!(
                    "file byte stream read failed: {error}"
                ))))
            }
        }
    }
}

pub fn file_write<B: RuntimeBytes + ?Sized>(file: &mut File, data: &B) -> Result<(), FileError> {
    file.inner
        .write_all(data.as_bytes_slice())
        .map_err(FileError::from)
}

pub fn file_write_string(file: &mut File, text: &str) -> Result<(), FileError> {
    file.inner
        .write_all(text.as_bytes())
        .map_err(FileError::from)
}

pub fn file_write_buffer(file: &mut File, buffer: &[u8]) -> Result<(), FileError> {
    file.inner.write_all(buffer).map_err(FileError::from)
}

pub fn file_read_all_async<P: RuntimePath + ?Sized>(
    path: &P,
) -> NativeAsyncPending<Result<Vec<u8>, FileError>> {
    let path = path.as_path().to_path_buf();
    spawn_tokio_native(async move { read_path_bounded_async(path).await })
}

pub fn file_read_all_string_async<P: RuntimePath + ?Sized>(
    path: &P,
) -> NativeAsyncPending<Result<String, FileError>> {
    let path = path.as_path().to_path_buf();
    spawn_tokio_native(async move { bytes_to_string(read_path_bounded_async(path).await?) })
}

pub fn file_write_async<P: RuntimePath + ?Sized, B: RuntimeBytes + ?Sized>(
    path: &P,
    data: &B,
) -> NativeAsyncPending<Result<(), FileError>> {
    let path = path.as_path().to_path_buf();
    let bytes = data.as_bytes_slice().to_vec();
    spawn_tokio_native(async move { tokio::fs::write(path, bytes).await.map_err(FileError::from) })
}

pub fn file_write_string_async<P: RuntimePath + ?Sized>(
    path: &P,
    text: &str,
) -> NativeAsyncPending<Result<(), FileError>> {
    let path = path.as_path().to_path_buf();
    let text = text.to_string();
    spawn_tokio_native(async move { tokio::fs::write(path, text).await.map_err(FileError::from) })
}

pub fn directory_list_files<P: RuntimePath + ?Sized>(path: &P) -> Result<Vec<String>, FileError> {
    let root = path.as_path();
    let mut files = Vec::new();
    collect_directory_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

pub fn directory_list_paths<P: RuntimePath + ?Sized>(path: &P) -> Result<Vec<PathBuf>, FileError> {
    let mut paths = Vec::new();
    let mut path_bytes = 0_usize;
    for entry in std::fs::read_dir(path.as_path())? {
        let path = entry?.path();
        ensure_directory_budget(paths.len() + 1, 1, &mut path_bytes, &path)?;
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

pub fn directory_exists<P: RuntimePath + ?Sized>(path: &P) -> bool {
    path.as_path().exists()
}

pub fn directory_is_file<P: RuntimePath + ?Sized>(path: &P) -> bool {
    path.as_path().is_file()
}

pub fn directory_is_dir<P: RuntimePath + ?Sized>(path: &P) -> bool {
    path.as_path().is_dir()
}

pub fn directory_create_all<P: RuntimePath + ?Sized>(path: &P) -> Result<(), FileError> {
    std::fs::create_dir_all(path.as_path()).map_err(FileError::from)
}

pub fn directory_create<P: RuntimePath + ?Sized>(path: &P) -> Result<(), FileError> {
    std::fs::create_dir(path.as_path()).map_err(FileError::from)
}

fn collect_directory_files(
    root: &std::path::Path,
    files: &mut Vec<String>,
) -> Result<(), FileError> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() {
        return Err(directory_symlink_error(root));
    }
    if root_metadata.is_file() {
        files.push(relative_runtime_path(root, root));
        return Ok(());
    }
    if !root_metadata.is_dir() {
        return Ok(());
    }

    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    let mut entries = 0_usize;
    let mut path_bytes = 0_usize;
    while let Some((current, depth)) = stack.pop() {
        if depth >= RUNTIME_DIRECTORY_MAX_DEPTH {
            return Err(directory_budget_error("depth", RUNTIME_DIRECTORY_MAX_DEPTH));
        }
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            entries = entries.saturating_add(1);
            ensure_directory_budget(entries, depth + 1, &mut path_bytes, &path)?;
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(directory_symlink_error(&path));
            }
            if metadata.is_dir() {
                stack.push((path, depth + 1));
            } else if metadata.is_file() {
                files.push(relative_runtime_path(root, &path));
            }
        }
    }
    Ok(())
}

fn ensure_directory_budget(
    entries: usize,
    depth: usize,
    path_bytes: &mut usize,
    path: &std::path::Path,
) -> Result<(), FileError> {
    if entries > RUNTIME_DIRECTORY_MAX_ENTRIES {
        return Err(directory_budget_error(
            "entry count",
            RUNTIME_DIRECTORY_MAX_ENTRIES,
        ));
    }
    if depth > RUNTIME_DIRECTORY_MAX_DEPTH {
        return Err(directory_budget_error("depth", RUNTIME_DIRECTORY_MAX_DEPTH));
    }
    *path_bytes = path_bytes.saturating_add(path.as_os_str().len());
    if *path_bytes > RUNTIME_DIRECTORY_MAX_PATH_BYTES {
        return Err(directory_budget_error(
            "path bytes",
            RUNTIME_DIRECTORY_MAX_PATH_BYTES,
        ));
    }
    Ok(())
}

fn directory_budget_error(kind: &str, limit: usize) -> FileError {
    FileError::new(
        "DirectoryLimitExceeded",
        &format!("directory traversal {kind} exceeds runtime limit of {limit}"),
    )
}

fn directory_symlink_error(path: &std::path::Path) -> FileError {
    FileError::new(
        "DirectorySymlink",
        &format!(
            "directory traversal does not follow symbolic link `{}`",
            path.display()
        ),
    )
}

fn relative_runtime_path(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn directory_remove_file<P: RuntimePath + ?Sized>(path: &P) -> Result<(), FileError> {
    std::fs::remove_file(path.as_path()).map_err(FileError::from)
}

pub fn directory_remove_dir_all<P: RuntimePath + ?Sized>(path: &P) -> Result<(), FileError> {
    std::fs::remove_dir_all(path.as_path()).map_err(FileError::from)
}

pub fn directory_copy_file<P: RuntimePath + ?Sized, Q: RuntimePath + ?Sized>(
    from: &P,
    to: &Q,
) -> Result<(), FileError> {
    std::fs::copy(from.as_path(), to.as_path())?;
    Ok(())
}

pub fn directory_rename<P: RuntimePath + ?Sized, Q: RuntimePath + ?Sized>(
    from: &P,
    to: &Q,
) -> Result<(), FileError> {
    std::fs::rename(from.as_path(), to.as_path()).map_err(FileError::from)
}

pub struct FileMetadata {
    pub is_file: bool,
    pub is_dir: bool,
    pub len: i64,
}

pub fn directory_metadata<P: RuntimePath + ?Sized>(path: &P) -> Result<FileMetadata, FileError> {
    let meta = std::fs::metadata(path.as_path())?;
    Ok(FileMetadata {
        is_file: meta.is_file(),
        is_dir: meta.is_dir(),
        len: meta.len() as i64,
    })
}

pub fn directory_read_string<P: RuntimePath + ?Sized>(path: &P) -> Result<String, FileError> {
    file_read_string(path)
}

fn read_path_bounded(path: &std::path::Path) -> Result<Vec<u8>, FileError> {
    let mut file = std::fs::File::open(path)?;
    read_file_remaining_bounded(&mut file)
}

fn read_file_remaining_bounded(file: &mut std::fs::File) -> Result<Vec<u8>, FileError> {
    read_file_remaining_with_budget(
        file,
        &ResourceBudget::new(RUNTIME_READ_CEILING_BYTES as u64),
    )
}

fn read_file_remaining_with_budget(
    file: &mut std::fs::File,
    budget: &ResourceBudget,
) -> Result<Vec<u8>, FileError> {
    let position = file.stream_position()?;
    let metadata_len = file
        .metadata()
        .ok()
        .map(|metadata| metadata.len().saturating_sub(position));
    read_bounded_with_budget(file, metadata_len, RUNTIME_READ_CEILING_BYTES, budget)
}

fn read_bounded_with_budget(
    reader: &mut impl Read,
    metadata_len: Option<u64>,
    ceiling: usize,
    budget: &ResourceBudget,
) -> Result<Vec<u8>, FileError> {
    let capacity = checked_read_capacity(metadata_len, ceiling)?;
    let capacity_reservation = if capacity == 0 {
        None
    } else {
        Some(budget.try_reserve(capacity).map_err(|error| {
            FileError::new(
                "ResourceBudget",
                &format!("file read byte budget exhausted: {error}"),
            )
        })?)
    };
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 8192];
    while bytes.len() < ceiling {
        let remaining = ceiling - bytes.len();
        let read_len = remaining.min(buffer.len());
        let read = reader.read(&mut buffer[..read_len])?;
        if read == 0 {
            if let Some(reservation) = capacity_reservation {
                reservation.commit(bytes.len());
            }
            return Ok(bytes);
        }
        if capacity_reservation.is_none() {
            budget.try_consume(read).map_err(|error| {
                FileError::new(
                    "ResourceBudget",
                    &format!("file read byte budget exhausted: {error}"),
                )
            })?;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let mut overflow_probe = [0_u8; 1];
    if reader.read(&mut overflow_probe)? != 0 {
        return Err(read_ceiling_error(ceiling.saturating_add(1), ceiling));
    }
    if let Some(reservation) = capacity_reservation {
        reservation.commit(bytes.len());
    }
    Ok(bytes)
}

async fn read_path_bounded_async(path: PathBuf) -> Result<Vec<u8>, FileError> {
    let mut file = tokio::fs::File::open(path).await?;
    let metadata_len = file.metadata().await.ok().map(|metadata| metadata.len());
    let capacity = checked_read_capacity(metadata_len, RUNTIME_READ_CEILING_BYTES)?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 8192];
    while bytes.len() < RUNTIME_READ_CEILING_BYTES {
        let remaining = RUNTIME_READ_CEILING_BYTES - bytes.len();
        let read_len = remaining.min(buffer.len());
        let read = file.read(&mut buffer[..read_len]).await?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let mut overflow_probe = [0_u8; 1];
    if file.read(&mut overflow_probe).await? != 0 {
        return Err(read_ceiling_error(
            RUNTIME_READ_CEILING_BYTES.saturating_add(1),
            RUNTIME_READ_CEILING_BYTES,
        ));
    }
    Ok(bytes)
}

fn checked_read_capacity(metadata_len: Option<u64>, ceiling: usize) -> Result<usize, FileError> {
    let Some(metadata_len) = metadata_len else {
        return Ok(0);
    };
    if metadata_len > ceiling as u64 {
        return Err(read_ceiling_error_u64(metadata_len, ceiling));
    }
    usize::try_from(metadata_len).map_err(|_| read_ceiling_error_u64(metadata_len, ceiling))
}

fn read_ceiling_error(actual: usize, ceiling: usize) -> FileError {
    read_ceiling_error_u64(actual as u64, ceiling)
}

fn read_ceiling_error_u64(actual: u64, ceiling: usize) -> FileError {
    FileError::new(
        "FileTooLarge",
        &format!("file read size {actual} exceeds runtime read ceiling of {ceiling} bytes"),
    )
}

fn bytes_to_string(bytes: Vec<u8>) -> Result<String, FileError> {
    String::from_utf8(bytes).map_err(|error| {
        FileError::from(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

pub fn directory_write_string<P: RuntimePath + ?Sized>(
    path: &P,
    content: &str,
) -> Result<(), FileError> {
    std::fs::write(path.as_path(), content.as_bytes()).map_err(FileError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_runtime::Executor;

    #[test]
    fn safe_relative_paths_reject_absolute_and_parent_traversal() {
        assert!(path_safe_relative("stdlib/json/json.rssi").is_ok());
        assert!(path_safe_relative("/tmp/file").is_err());
        assert!(path_safe_relative("../secret").is_err());
        assert!(path_safe_relative("stdlib/../secret").is_err());

        let root = std::env::temp_dir();
        let resolved = path_resolve_relative(&root, "rsscript-safe-path.txt")
            .expect("relative path should resolve under root");
        assert!(resolved.starts_with(root));
    }

    #[test]
    fn async_file_text_io_uses_tokio_native_pending() {
        let path =
            std::env::temp_dir().join(format!("rsscript-async-file-{}.txt", std::process::id()));
        let mut executor = Executor::new();

        executor
            .run_pending(file_write_string_async(&path, "hello async file"))
            .expect("async write should succeed");
        let text = executor
            .run_pending(file_read_all_string_async(&path))
            .expect("async read should succeed");

        assert_eq!(text, "hello async file");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn direct_file_helpers_cover_common_script_io() {
        let path =
            std::env::temp_dir().join(format!("rsscript-direct-file-{}.txt", std::process::id()));

        assert!(!file_exists(&path));
        file_write_string_to_path(&path, "hello").expect("write should succeed");
        file_append_string(&path, " world").expect("append should succeed");

        assert!(file_exists(&path));
        assert_eq!(
            file_read_string(&path).expect("read string should succeed"),
            "hello world"
        );
        assert_eq!(
            file_read_bytes(&path).expect("read bytes should succeed"),
            b"hello world"
        );
        assert!(path_is_absolute(&path));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn oversized_file_reads_are_rejected_from_metadata() {
        let path =
            std::env::temp_dir().join(format!("rsscript-oversized-file-{}", std::process::id()));
        let file = std::fs::File::create(&path).expect("test file should be created");
        file.set_len(RUNTIME_READ_CEILING_BYTES as u64 + 1)
            .expect("sparse test file should be sized");
        drop(file);

        let error = file_read_bytes(&path).expect_err("oversized file should be rejected");

        assert_eq!(error.kind(), "FileTooLarge");
        assert!(error.message().contains("runtime read ceiling"));
        let mut file = file_open_read(&path).expect("oversized file should still open");
        assert_eq!(
            file_read_all(&mut file)
                .expect_err("oversized open file should be rejected")
                .kind(),
            "FileTooLarge"
        );
        assert_eq!(
            Executor::new()
                .run_pending(file_read_all_async(&path))
                .expect_err("oversized async file should be rejected")
                .kind(),
            "FileTooLarge"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_open_file_read_restores_cursor() {
        let path =
            std::env::temp_dir().join(format!("rsscript-cursor-file-{}", std::process::id()));
        let file = std::fs::File::create(&path).expect("test file should be created");
        file.set_len(RUNTIME_READ_CEILING_BYTES as u64 + 1)
            .expect("sparse test file should be sized");
        drop(file);
        let mut file = file_open_read(&path).expect("file should open");

        file_read_all(&mut file).expect_err("oversized read should fail");

        assert_eq!(
            file.inner
                .stream_position()
                .expect("cursor should be readable"),
            0
        );
        assert_eq!(
            file_read_bytes_from_offset(&path, RUNTIME_READ_CEILING_BYTES as u64)
                .expect("bounded tail should be readable")
                .len(),
            1
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn bounded_reader_rejects_data_beyond_reported_size() {
        let mut reader = std::io::Cursor::new(b"12345");

        let error = read_bounded_with_budget(&mut reader, Some(4), 4, &ResourceBudget::new(4))
            .expect_err("reader growth beyond metadata should be rejected");

        assert_eq!(error.kind(), "FileTooLarge");
    }

    #[test]
    fn file_reads_share_a_cumulative_byte_budget() {
        let path =
            std::env::temp_dir().join(format!("rsscript-budget-file-{}", std::process::id()));
        file_write_string_to_path(&path, "1234").expect("test file should write");
        let budget = ResourceBudget::new(6);

        assert_eq!(
            file_read_bytes_with_budget(&path, &budget).expect("first read should fit"),
            b"1234"
        );
        let error = file_read_bytes_with_budget(&path, &budget)
            .expect_err("second read should exceed the shared budget");
        assert_eq!(error.kind(), "ResourceBudget");
        assert!(error.message().contains("byte budget exhausted"));
        assert_eq!(budget.bytes_used(), 4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_byte_stream_rejects_oversized_chunks_before_opening() {
        let missing = std::env::temp_dir().join("rsscript-missing-stream-input");

        let error = match file_bytes_stream(&missing, RUNTIME_READ_CEILING_BYTES as i64 + 1) {
            Ok(_) => panic!("oversized chunks should be rejected"),
            Err(error) => error,
        };
        let message = crate::channel_error_message(&error);

        assert!(message.contains("chunk size"));
        assert!(message.contains("runtime read ceiling"));
    }

    #[test]
    #[cfg(unix)]
    fn recursive_directory_listing_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("rsscript-directory-cycle-{}", std::process::id()));
        std::fs::create_dir_all(root.join("child")).expect("directory should be created");
        symlink(&root, root.join("child").join("cycle")).expect("symlink should be created");

        let error = directory_list_files(&root).expect_err("symlink traversal should be rejected");

        assert_eq!(error.kind(), "DirectorySymlink");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recursive_directory_listing_enforces_depth_budget() {
        let root =
            std::env::temp_dir().join(format!("rsscript-directory-depth-{}", std::process::id()));
        let mut current = root.clone();
        for _ in 0..=RUNTIME_DIRECTORY_MAX_DEPTH {
            current.push("d");
            std::fs::create_dir_all(&current).expect("nested directory should be created");
        }

        let error =
            directory_list_files(&root).expect_err("deep traversal should exceed the budget");

        assert_eq!(error.kind(), "DirectoryLimitExceeded");
        assert!(error.message().contains("depth"));
        let _ = std::fs::remove_dir_all(root);
    }
}
