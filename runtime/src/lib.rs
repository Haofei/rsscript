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
}

impl<T> Gc<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
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
}

impl<T: fmt::Debug> fmt::Debug for Gc<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Gc").field(&self.read()).finish()
    }
}

pub fn manage<T>(value: T) -> Gc<T> {
    Gc::new(value)
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

    pub fn borrow(&self) -> ResourceLease<'_, T> {
        ResourceLease {
            values: self.values.borrow_mut(),
            index: 0,
        }
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
    fn resource_pool_borrows_scoped_resource() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        let lease = pool.borrow();

        assert_eq!(lease.0, 7);
    }
}
