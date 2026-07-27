use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::{Rc, Weak};

use crate::error::{
    RuntimeError, SourceSpan, managed_read_error, managed_write_error, panic_runtime_error,
};

#[derive(Clone)]
pub struct Managed<T> {
    inner: Rc<RefCell<T>>,
    origin_span: Option<SourceSpan>,
}

impl<T> Managed<T> {
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

    pub fn try_read(&self) -> Result<ManagedRead<'_, T>, RuntimeError> {
        self.inner
            .try_borrow()
            .map(ManagedRead)
            .map_err(managed_read_error)
    }

    pub fn try_write(&self) -> Result<ManagedWrite<'_, T>, RuntimeError> {
        self.inner
            .try_borrow_mut()
            .map(ManagedWrite)
            .map_err(managed_write_error)
    }

    pub fn try_read_at(&self, span: SourceSpan) -> Result<ManagedRead<'_, T>, RuntimeError> {
        self.try_read().map_err(|error| error.with_span(span))
    }

    pub fn try_write_at(&self, span: SourceSpan) -> Result<ManagedWrite<'_, T>, RuntimeError> {
        self.try_write().map_err(|error| error.with_span(span))
    }

    pub fn read(&self) -> ManagedRead<'_, T> {
        match self.try_read() {
            Ok(value) => value,
            Err(error) => panic_runtime_error(error),
        }
    }

    pub fn write(&self) -> ManagedWrite<'_, T> {
        match self.try_write() {
            Ok(value) => value,
            Err(error) => panic_runtime_error(error),
        }
    }

    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        Rc::ptr_eq(&left.inner, &right.inner)
    }

    pub fn origin_span(&self) -> Option<&SourceSpan> {
        self.origin_span.as_ref()
    }
}

#[derive(Clone)]
pub struct WeakManaged<T> {
    inner: Weak<RefCell<T>>,
    origin_span: Option<SourceSpan>,
}

impl<T> WeakManaged<T> {
    pub fn upgrade(&self) -> Option<Managed<T>> {
        self.inner.upgrade().map(|inner| Managed {
            inner,
            origin_span: self.origin_span.clone(),
        })
    }

    pub fn origin_span(&self) -> Option<&SourceSpan> {
        self.origin_span.as_ref()
    }
}

impl<T: fmt::Debug> fmt::Debug for WeakManaged<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.upgrade() {
            Some(value) => match value.try_read() {
                Ok(read) => formatter.debug_tuple("WeakManaged").field(&read).finish(),
                Err(_) => formatter
                    .debug_tuple("WeakManaged")
                    .field(&"<borrow conflict>")
                    .finish(),
            },
            None => formatter.write_str("WeakManaged(<dropped>)"),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Managed<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_read() {
            Ok(read) => formatter.debug_tuple("Managed").field(&read).finish(),
            Err(_) => formatter
                .debug_tuple("Managed")
                .field(&"<borrow conflict>")
                .finish(),
        }
    }
}

pub fn manage<T>(value: T) -> Managed<T> {
    Managed::new(value)
}

pub fn manage_at<T>(value: T, span: SourceSpan) -> Managed<T> {
    Managed::new_at(value, span)
}

pub fn weak<T>(value: &Managed<T>) -> WeakManaged<T> {
    WeakManaged {
        inner: Rc::downgrade(&value.inner),
        origin_span: value.origin_span.clone(),
    }
}

pub fn unwrap_runtime<T>(result: Result<T, RuntimeError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic_runtime_error(error),
    }
}

pub struct ManagedRead<'a, T>(Ref<'a, T>);

impl<T> Deref for ManagedRead<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for ManagedRead<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

pub struct ManagedWrite<'a, T>(RefMut<'a, T>);

impl<T> Deref for ManagedWrite<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for ManagedWrite<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for ManagedWrite<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_formatting_does_not_panic_during_write_borrow() {
        let value = manage(String::from("value"));
        let weak_value = weak(&value);
        let _write = value.try_write().expect("write borrow should succeed");

        assert!(format!("{value:?}").contains("borrow conflict"));
        assert!(format!("{weak_value:?}").contains("borrow conflict"));
    }
}
