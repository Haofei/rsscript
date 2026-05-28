use std::cell::{BorrowError, BorrowMutError, Ref, RefCell, RefMut};
use std::fmt;
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::Rc;

pub trait Managed {}

impl<T: 'static> Managed for T {}

pub trait Resource {}

pub struct File {
    inner: std::fs::File,
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

pub fn file_write<B: RuntimeBytes + ?Sized>(file: &mut File, data: &B) -> std::io::Result<()> {
    file.inner.write_all(data.as_bytes_slice())
}

#[derive(Clone)]
pub struct Gc<T> {
    inner: Rc<RefCell<T>>,
    origin_span: Option<SourceSpan>,
}

impl<T> Gc<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
            origin_span: None,
        }
    }

    pub fn new_at(value: T, span: SourceSpan) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
            origin_span: Some(span),
        }
    }

    pub fn try_read(&self) -> Result<GcRead<'_, T>, RuntimeError> {
        self.inner
            .try_borrow()
            .map(GcRead)
            .map_err(RuntimeError::from)
    }

    pub fn try_write(&self) -> Result<GcWrite<'_, T>, RuntimeError> {
        self.inner
            .try_borrow_mut()
            .map(GcWrite)
            .map_err(RuntimeError::from)
    }

    pub fn try_read_at(&self, span: SourceSpan) -> Result<GcRead<'_, T>, RuntimeError> {
        self.try_read().map_err(|error| error.with_span(span))
    }

    pub fn try_write_at(&self, span: SourceSpan) -> Result<GcWrite<'_, T>, RuntimeError> {
        self.try_write().map_err(|error| error.with_span(span))
    }

    pub fn read(&self) -> GcRead<'_, T> {
        self.try_read()
            .expect("RSScript runtime read conflict should be reported through diagnostics")
    }

    pub fn write(&self) -> GcWrite<'_, T> {
        self.try_write()
            .expect("RSScript runtime write conflict should be reported through diagnostics")
    }

    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        Rc::ptr_eq(&left.inner, &right.inner)
    }

    pub fn origin_span(&self) -> Option<&SourceSpan> {
        self.origin_span.as_ref()
    }
}

impl<T: fmt::Debug> fmt::Debug for Gc<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Gc").field(&self.read()).finish()
    }
}

pub fn manage<T>(value: T) -> Gc<T> {
    Gc::new(value)
}

pub fn manage_at<T>(value: T, span: SourceSpan) -> Gc<T> {
    Gc::new_at(value, span)
}

pub fn unwrap_runtime<T>(result: Result<T, RuntimeError>) -> T {
    result.expect("RSScript runtime error should be reported through diagnostics")
}

pub fn log_write(message: &str) {
    println!("{message}");
}

pub fn assert_equal(left: &str, right: &str) {
    assert_eq!(left, right);
}

pub struct GcRead<'a, T>(Ref<'a, T>);

impl<T> Deref for GcRead<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for GcRead<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

pub struct GcWrite<'a, T>(RefMut<'a, T>);

impl<T> Deref for GcWrite<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for GcWrite<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for GcWrite<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    ManagedReadConflict,
    ManagedWriteConflict,
    ResourcePoolBorrowConflict,
    ResourcePoolEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub span: Option<SourceSpan>,
}

impl RuntimeError {
    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

impl From<BorrowError> for RuntimeError {
    fn from(_: BorrowError) -> Self {
        Self {
            kind: RuntimeErrorKind::ManagedReadConflict,
            message: "managed value is already mutably borrowed".to_string(),
            span: None,
        }
    }
}

impl From<BorrowMutError> for RuntimeError {
    fn from(_: BorrowMutError) -> Self {
        Self {
            kind: RuntimeErrorKind::ManagedWriteConflict,
            message: "managed value is already borrowed".to_string(),
            span: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    pub file: &'static str,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl SourceSpan {
    pub const fn new(file: &'static str, line: usize, column: usize, length: usize) -> Self {
        Self {
            file,
            line,
            column,
            length,
        }
    }
}

#[derive(Debug)]
pub struct ResourcePool<T: Resource> {
    values: RefCell<Vec<T>>,
}

impl<T: Resource> ResourcePool<T> {
    pub fn new(values: Vec<T>) -> Self {
        Self {
            values: RefCell::new(values),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub fn push(&self, value: T) {
        self.values.borrow_mut().push(value);
    }

    pub fn try_borrow(&self) -> Result<ResourceLease<'_, T>, RuntimeError> {
        let values = self.values.try_borrow_mut().map_err(|_| RuntimeError {
            kind: RuntimeErrorKind::ResourcePoolBorrowConflict,
            message: "resource pool is already borrowed".to_string(),
            span: None,
        })?;
        if values.is_empty() {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::ResourcePoolEmpty,
                message: "resource pool has no available resources".to_string(),
                span: None,
            });
        }
        Ok(ResourceLease { values, index: 0 })
    }

    pub fn try_borrow_at(&self, span: SourceSpan) -> Result<ResourceLease<'_, T>, RuntimeError> {
        self.try_borrow().map_err(|error| error.with_span(span))
    }

    pub fn borrow(&self) -> ResourceLease<'_, T> {
        self.try_borrow()
            .expect("RSScript resource pool conflict should be reported through diagnostics")
    }
}

pub struct ResourceLease<'a, T: Resource> {
    values: RefMut<'a, Vec<T>>,
    index: usize,
}

impl<T: Resource> Deref for ResourceLease<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.values[self.index]
    }
}

impl<T: Resource> DerefMut for ResourceLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values[self.index]
    }
}

impl<T: Resource> fmt::Debug for ResourceLease<'_, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ResourceLease")
            .field(&self.values[self.index])
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Resource, ResourcePool, RuntimeErrorKind, manage};

    #[derive(Debug)]
    struct FileHandle(i32);

    impl Resource for FileHandle {}

    #[test]
    fn manage_wraps_value_in_gc_handle() {
        let value = manage(String::from("cached"));

        assert_eq!(&*value.read(), "cached");
    }

    #[test]
    fn managed_aliases_observe_mutation() {
        let left = manage(String::from("cached"));
        let right = left.clone();

        right.write().push_str("-updated");

        assert_eq!(&*left.read(), "cached-updated");
        assert!(super::Gc::ptr_eq(&left, &right));
    }

    #[test]
    fn managed_conflicts_report_runtime_errors() {
        let value = manage(String::from("cached"));
        let _write = value.try_write().expect("initial write should succeed");
        let error = value
            .try_read()
            .expect_err("read should conflict with write");

        assert_eq!(error.kind, RuntimeErrorKind::ManagedReadConflict);
    }

    #[test]
    fn managed_handles_keep_origin_span() {
        let span = super::SourceSpan::new("cache.rss", 3, 9, 6);
        let value = super::manage_at(String::from("cached"), span.clone());

        assert_eq!(value.origin_span(), Some(&span));
    }

    #[test]
    fn managed_conflicts_can_attach_operation_span() {
        let value = manage(String::from("cached"));
        let _write = value.try_write().expect("initial write should succeed");
        let span = super::SourceSpan::new("cache.rss", 4, 12, 5);
        let error = value
            .try_read_at(span.clone())
            .expect_err("read should conflict with write");

        assert_eq!(error.span, Some(span));
    }

    #[test]
    fn resource_pool_borrows_scoped_resource() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        let lease = pool.borrow();

        assert_eq!(lease.0, 7);
    }

    #[test]
    fn resource_pool_reports_empty_pool() {
        let pool = ResourcePool::<FileHandle>::empty();
        let error = pool.try_borrow().expect_err("empty pool should error");

        assert_eq!(error.kind, RuntimeErrorKind::ResourcePoolEmpty);
    }

    #[test]
    fn resource_pool_reports_borrow_conflict() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        let _lease = pool.try_borrow().expect("initial borrow should succeed");
        let error = pool
            .try_borrow()
            .expect_err("second borrow should conflict");

        assert_eq!(error.kind, RuntimeErrorKind::ResourcePoolBorrowConflict);
    }

    #[test]
    fn file_runtime_hooks_write_and_read_bytes() {
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-file-{}.txt", std::process::id()));

        {
            let mut file = super::file_open_write(&path).expect("file should open for write");
            super::file_write(&mut file, &"hello file").expect("write should succeed");
        }

        let mut file = super::file_open_read(&path).expect("file should open for read");
        let bytes = super::file_read_all(&mut file).expect("read should succeed");

        assert_eq!(bytes, b"hello file");
        let _ = std::fs::remove_file(path);
    }
}
