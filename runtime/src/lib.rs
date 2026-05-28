use std::cell::{RefCell, RefMut};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

pub trait Managed {}

impl<T: 'static> Managed for T {}

pub trait Resource {}

#[derive(Clone)]
pub struct Gc<T> {
    inner: Rc<T>,
}

impl<T> Gc<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(value),
        }
    }

    pub fn get(&self) -> &T {
        &self.inner
    }

    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        Rc::ptr_eq(&left.inner, &right.inner)
    }
}

impl<T> Deref for Gc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T: fmt::Debug> fmt::Debug for Gc<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Gc").field(&self.inner).finish()
    }
}

pub fn manage<T>(value: T) -> Gc<T> {
    Gc::new(value)
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
    use super::{Resource, ResourcePool, manage};

    #[derive(Debug)]
    struct FileHandle(i32);

    impl Resource for FileHandle {}

    #[test]
    fn manage_wraps_value_in_gc_handle() {
        let value = manage(String::from("cached"));

        assert_eq!(value.get(), "cached");
    }

    #[test]
    fn resource_pool_borrows_scoped_resource() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        let lease = pool.borrow();

        assert_eq!(lease.0, 7);
    }
}
