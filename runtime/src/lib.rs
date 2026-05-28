use std::cell::{BorrowError, BorrowMutError, Ref, RefCell, RefMut};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

pub trait Managed {}

impl<T: 'static> Managed for T {}

pub trait Resource {}

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
}
