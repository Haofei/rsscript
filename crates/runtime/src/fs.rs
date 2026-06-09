use std::io::{Read, Write};
use std::path::PathBuf;

use crate::async_runtime::{NativeAsyncPending, spawn_tokio_native};
use crate::channel::{ChannelError, RssStream, stream_from_iterator};
use crate::diagnostics::Resource;

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
    std::fs::read(path.as_path()).map_err(FileError::from)
}

pub fn file_read_string<P: RuntimePath + ?Sized>(path: &P) -> Result<String, FileError> {
    std::fs::read_to_string(path.as_path()).map_err(FileError::from)
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
    let mut bytes = Vec::new();
    file.inner.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn file_read_all_string(file: &mut File) -> Result<String, FileError> {
    let mut text = String::new();
    file.inner.read_to_string(&mut text)?;
    Ok(text)
}

pub fn file_read_into(file: &mut File, buffer: &mut Vec<u8>) -> Result<bool, FileError> {
    buffer.clear();
    let bytes_read = file.inner.read_to_end(buffer)?;
    Ok(bytes_read > 0)
}

pub fn file_bytes_stream<P: RuntimePath + ?Sized>(
    path: &P,
    chunk_size: i64,
) -> Result<RssStream<Vec<u8>>, ChannelError> {
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
    spawn_tokio_native(async move { tokio::fs::read(path).await.map_err(FileError::from) })
}

pub fn file_read_all_string_async<P: RuntimePath + ?Sized>(
    path: &P,
) -> NativeAsyncPending<Result<String, FileError>> {
    let path = path.as_path().to_path_buf();
    spawn_tokio_native(async move {
        tokio::fs::read_to_string(path)
            .await
            .map_err(FileError::from)
    })
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
    collect_directory_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

pub fn directory_list_paths<P: RuntimePath + ?Sized>(path: &P) -> Result<Vec<PathBuf>, FileError> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(path.as_path())? {
        paths.push(entry?.path());
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
    current: &std::path::Path,
    files: &mut Vec<String>,
) -> Result<(), FileError> {
    if current.is_file() {
        files.push(relative_runtime_path(root, current));
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_directory_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(relative_runtime_path(root, &path));
        }
    }
    Ok(())
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
    std::fs::read_to_string(path.as_path()).map_err(FileError::from)
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
}
