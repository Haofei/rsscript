use std::io::{Read, Write};
use std::path::PathBuf;

use crate::diagnostics::Resource;

pub struct File {
    pub(crate) inner: std::fs::File,
}

impl Resource for File {}

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

pub trait RuntimeBytes {
    fn as_bytes_slice(&self) -> &[u8];
}

impl RuntimeBytes for Vec<u8> {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_slice()
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

pub fn file_error_message(error: &std::io::Error) -> String {
    error.to_string()
}

pub fn file_open<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<File> {
    file_open_read(path)
}

pub fn file_open_read<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<File> {
    std::fs::File::open(path.as_path()).map(|inner| File { inner })
}

pub fn file_open_write<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<File> {
    std::fs::File::create(path.as_path()).map(|inner| File { inner })
}

pub fn file_read_all(file: &mut File) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.inner.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn file_read_all_string(file: &mut File) -> std::io::Result<String> {
    let mut text = String::new();
    file.inner.read_to_string(&mut text)?;
    Ok(text)
}

pub fn file_read_into(file: &mut File, buffer: &mut Vec<u8>) -> std::io::Result<bool> {
    buffer.clear();
    let bytes_read = file.inner.read_to_end(buffer)?;
    Ok(bytes_read > 0)
}

pub fn file_write<B: RuntimeBytes + ?Sized>(file: &mut File, data: &B) -> std::io::Result<()> {
    file.inner.write_all(data.as_bytes_slice())
}

pub fn file_write_string(file: &mut File, text: &str) -> std::io::Result<()> {
    file.inner.write_all(text.as_bytes())
}

pub fn file_write_buffer(file: &mut File, buffer: &[u8]) -> std::io::Result<()> {
    file.inner.write_all(buffer)
}

pub fn directory_list_files<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<Vec<String>> {
    let root = path.as_path();
    let mut files = Vec::new();
    collect_directory_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
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

pub fn directory_create_all<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<()> {
    std::fs::create_dir_all(path.as_path())
}

fn collect_directory_files(
    root: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
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
