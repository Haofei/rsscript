use std::cell::RefCell;
use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::diagnostics::Resource;
use crate::error::{RuntimeError, RuntimeErrorKind, SourceSpan, panic_runtime_error};

/// Borrow-time pool failure surfaced to RSScript: either the pool is exhausted
/// (all `max_size` resources are checked out) or a lazy factory failed to create
/// one. Opaque, like `ChannelError`; `?` propagation and `message()` are the
/// intended patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolError {
    message: String,
}

impl PoolError {
    pub fn exhausted() -> Self {
        Self {
            message: "resource pool is exhausted".to_string(),
        }
    }

    pub fn factory_failed(message: impl Into<String>) -> Self {
        Self {
            message: format!("resource pool factory failed: {}", message.into()),
        }
    }

    pub fn invalid_capacity(max_size: i64) -> Self {
        Self {
            message: format!("resource pool max_size must be positive, got {max_size}"),
        }
    }

    pub fn factory_reentrant() -> Self {
        Self {
            message: "resource pool factory re-entered the same pool".to_string(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub fn pool_error_message(error: &PoolError) -> String {
    error.message.clone()
}

/// A snapshot of pool occupancy, returned by `ResourcePool.stats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    capacity: i64,
    created: i64,
    available: i64,
    in_use: i64,
}

pub fn pool_stats_capacity(stats: &PoolStats) -> i64 {
    stats.capacity
}

pub fn pool_stats_created(stats: &PoolStats) -> i64 {
    stats.created
}

pub fn pool_stats_available(stats: &PoolStats) -> i64 {
    stats.available
}

pub fn pool_stats_in_use(stats: &PoolStats) -> i64 {
    stats.in_use
}

pub fn pool_stats<T: Resource>(pool: &ResourcePool<T>) -> PoolStats {
    pool.stats()
}

type LazyFactory<T> = Box<dyn FnMut() -> Result<T, PoolError>>;

struct PoolState<T: Resource> {
    /// Returned, ready-to-reuse resources.
    idle: Vec<T>,
    /// Resources currently checked out via a live lease.
    in_use: usize,
    /// Live resources (`idle.len() + in_use`).
    created: usize,
    /// Hard cap on `created`.
    max_size: usize,
    /// Present iff the pool creates resources on demand (lazy).
    factory: Option<LazyFactory<T>>,
    /// True only while the lazy factory is executing outside the `RefCell`
    /// borrow. A recursive borrow is rejected instead of panicking or creating
    /// past the pool cap.
    factory_active: bool,
    /// Set when a lazy pool was constructed with a non-positive `max_size`: such a
    /// pool can never hand out a resource, so `try_borrow` reports this explicitly
    /// (`PoolError::invalid_capacity`) rather than masquerading as "exhausted".
    invalid_capacity: Option<i64>,
}

pub struct ResourcePool<T: Resource> {
    state: RefCell<PoolState<T>>,
}

impl<T: Resource> ResourcePool<T> {
    fn from_state(state: PoolState<T>) -> Self {
        Self {
            state: RefCell::new(state),
        }
    }

    /// Eager pool seeded with explicit resources (used by tests).
    pub fn new(values: Vec<T>) -> Self {
        let created = values.len();
        Self::from_state(PoolState {
            idle: values,
            in_use: 0,
            created,
            max_size: created,
            factory: None,
            factory_active: false,
            invalid_capacity: None,
        })
    }

    /// Eager infallible: create exactly `max_size` resources up front.
    pub fn from_factory<F>(max_size: i64, mut create: F) -> Self
    where
        F: FnMut() -> T,
    {
        let count = max_size.max(0) as usize;
        let idle: Vec<T> = (0..count).map(|_| create()).collect();
        Self::new(idle)
    }

    /// Eager fallible: create `max_size` resources up front, short-circuiting on
    /// the first factory error (the user's typed `E`).
    pub fn try_from_factory<E, F>(max_size: i64, mut create: F) -> Result<Self, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        let count = max_size.max(0) as usize;
        let mut idle = Vec::with_capacity(count);
        for _ in 0..count {
            idle.push(create()?);
        }
        Ok(Self::new(idle))
    }

    /// Lazy infallible: create resources on demand, up to `max_size`.
    pub fn lazy_from_factory<F>(max_size: i64, mut create: F) -> Self
    where
        F: FnMut() -> T + 'static,
    {
        let factory: LazyFactory<T> = Box::new(move || Ok(create()));
        Self::from_state(PoolState {
            idle: Vec::new(),
            in_use: 0,
            created: 0,
            max_size: max_size.max(0) as usize,
            factory: Some(factory),
            factory_active: false,
            invalid_capacity: (max_size <= 0).then_some(max_size),
        })
    }

    /// Lazy fallible: create resources on demand; factory failures surface as
    /// `PoolError` at `try_borrow` time.
    pub fn try_lazy_from_factory<F>(max_size: i64, create: F) -> Self
    where
        F: FnMut() -> Result<T, PoolError> + 'static,
    {
        Self::from_state(PoolState {
            idle: Vec::new(),
            in_use: 0,
            created: 0,
            max_size: max_size.max(0) as usize,
            factory: Some(Box::new(create)),
            factory_active: false,
            invalid_capacity: (max_size <= 0).then_some(max_size),
        })
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Check out a resource: reuse an idle one, else lazily create one if under
    /// `max_size`, else report exhaustion. The accounting supports several live
    /// leases, though the language currently gates pools to one active lease (RS0709).
    pub fn try_borrow(&self) -> Result<ResourceLease<'_, T>, PoolError> {
        let mut factory = {
            let mut state = self.state.borrow_mut();
            if let Some(max_size) = state.invalid_capacity {
                return Err(PoolError::invalid_capacity(max_size));
            }
            if state.factory_active {
                return Err(PoolError::factory_reentrant());
            }
            if let Some(value) = state.idle.pop() {
                state.in_use += 1;
                return Ok(ResourceLease {
                    pool: self,
                    value: Some(value),
                    discarded: false,
                });
            }
            if state.created >= state.max_size {
                return Err(PoolError::exhausted());
            }
            let Some(factory) = state.factory.take() else {
                return Err(PoolError::exhausted());
            };
            state.factory_active = true;
            factory
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut factory));
        let mut state = self.state.borrow_mut();
        state.factory = Some(factory);
        state.factory_active = false;
        let value = match result {
            Ok(result) => result?,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        state.created += 1;
        state.in_use += 1;
        drop(state);
        Ok(ResourceLease {
            pool: self,
            value: Some(value),
            discarded: false,
        })
    }

    /// The infallible `with pool.borrow() as x` path: exhaustion panics with a
    /// runtime diagnostic carrying the `with` site span.
    pub fn borrow_at(pool: &Self, span: SourceSpan) -> Result<ResourceLease<'_, T>, RuntimeError> {
        pool.try_borrow().map_err(|error| {
            RuntimeError {
                kind: RuntimeErrorKind::ResourcePoolEmpty,
                message: error.message,
                span: None,
            }
            .with_span(span)
        })
    }

    pub fn borrow(&self) -> ResourceLease<'_, T> {
        match self.try_borrow() {
            Ok(lease) => lease,
            Err(error) => panic_runtime_error(RuntimeError {
                kind: RuntimeErrorKind::ResourcePoolEmpty,
                message: error.message,
                span: None,
            }),
        }
    }

    pub fn stats(&self) -> PoolStats {
        let state = self.state.borrow();
        PoolStats {
            capacity: state.max_size as i64,
            created: state.created as i64,
            available: state.idle.len() as i64,
            in_use: state.in_use as i64,
        }
    }

    fn check_in(&self, value: T) {
        let mut state = self.state.borrow_mut();
        state.in_use = state.in_use.saturating_sub(1);
        state.idle.push(value);
    }

    fn evict(&self) {
        let mut state = self.state.borrow_mut();
        state.in_use = state.in_use.saturating_sub(1);
        state.created = state.created.saturating_sub(1);
    }
}

pub struct ResourceLease<'a, T: Resource> {
    pool: &'a ResourcePool<T>,
    /// The checked-out resource; `None` only transiently during return.
    value: Option<T>,
    /// When set, the resource is dropped on scope exit instead of returned, so a
    /// lazy pool can recreate it (transaction rollback / broken-resource eviction).
    discarded: bool,
}

/// Mark a leased resource broken so it is evicted (not returned to the pool) when
/// the lease scope ends. A lazy pool can then create a fresh one on the next
/// `try_borrow`.
pub fn resource_lease_discard<T: Resource>(lease: &mut ResourceLease<'_, T>) {
    lease.discarded = true;
}

impl<T: Resource> Deref for ResourceLease<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value
            .as_ref()
            .expect("resource lease used after return")
    }
}

impl<T: Resource> DerefMut for ResourceLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
            .as_mut()
            .expect("resource lease used after return")
    }
}

impl<T: Resource> Drop for ResourceLease<'_, T> {
    fn drop(&mut self) {
        let Some(value) = self.value.take() else {
            return;
        };
        if self.discarded {
            drop(value);
            self.pool.evict();
        } else {
            self.pool.check_in(value);
        }
    }
}

impl<T: Resource> fmt::Debug for ResourceLease<'_, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ResourceLease")
            .field(&self.value)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::error::assertion_failed_error;
    use crate::*;

    #[derive(Debug)]
    struct FileHandle(i32);

    impl Resource for FileHandle {}

    #[test]
    fn manage_wraps_value_in_managed_handle() {
        let value = manage(String::from("cached"));

        assert_eq!(&*value.read(), "cached");
    }

    #[test]
    fn weak_handles_upgrade_while_managed_value_is_alive() {
        let value = manage(String::from("cached"));
        let weak = crate::weak(&value);

        assert_eq!(
            &*weak.upgrade().expect("value should still be live").read(),
            "cached"
        );

        drop(value);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn managed_aliases_observe_mutation() {
        let left = manage(String::from("cached"));
        let right = left.clone();

        right.write().push_str("-updated");

        assert_eq!(&*left.read(), "cached-updated");
        assert!(crate::Managed::ptr_eq(&left, &right));
    }

    #[test]
    fn managed_handles_are_single_isolate_values() {
        static_assertions::assert_not_impl_any!(crate::Managed<String>: Send, Sync);
        static_assertions::assert_not_impl_any!(crate::WeakManaged<String>: Send, Sync);
    }

    #[test]
    fn runtime_surface_has_no_legacy_gc_aliases() {
        let source = include_str!("lib.rs");
        let managed_alias = ["pub type ", "Gc"].concat();
        let read_alias = ["pub type ", "G", "c", "Read"].concat();
        let write_alias = ["pub type ", "G", "c", "Write"].concat();

        assert!(!source.contains(&managed_alias));
        assert!(!source.contains(&read_alias));
        assert!(!source.contains(&write_alias));
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
        let span = crate::SourceSpan::new("cache.rss", 3, 9, 6);
        let value = crate::manage_at(String::from("cached"), span.clone());

        assert_eq!(value.origin_span(), Some(&span));
    }

    #[test]
    fn path_runtime_hook_builds_pathbuf_from_string() {
        let path = crate::path_from_string("fixtures/rsscript-path.txt");

        assert_eq!(path, std::path::PathBuf::from("fixtures/rsscript-path.txt"));
        assert_eq!(
            crate::path_to_string(&path),
            std::path::PathBuf::from("fixtures/rsscript-path.txt")
                .to_string_lossy()
                .to_string()
        );
        assert_eq!(
            crate::path_file_name(&path).as_deref(),
            Some("rsscript-path.txt")
        );
        assert_eq!(crate::path_extension(&path).as_deref(), Some("txt"));
        assert_eq!(
            crate::path_parent(&path),
            Some(std::path::PathBuf::from("fixtures"))
        );
    }

    #[test]
    fn managed_conflicts_can_attach_operation_span() {
        let value = manage(String::from("cached"));
        let _write = value.try_write().expect("initial write should succeed");
        let span = crate::SourceSpan::new("cache.rss", 4, 12, 5);
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
    fn resource_pool_reports_exhaustion() {
        let pool = ResourcePool::<FileHandle>::empty();
        let error = pool.try_borrow().expect_err("empty pool should error");

        assert!(error.message().contains("exhausted"), "{}", error.message());
    }

    #[test]
    fn resource_pool_tracks_in_use_and_available() {
        // Internal lease accounting: each checkout decrements `available` and
        // increments `in_use`. NOTE: this exercises the runtime mechanism only —
        // the RSScript language currently permits one active lease per pool
        // (RS0709), so holding two leases at once is not reachable from RSScript.
        let pool = ResourcePool::new(vec![FileHandle(7), FileHandle(8)]);
        let first = pool.try_borrow().expect("first borrow should succeed");
        let second = pool.try_borrow().expect("second borrow should succeed");

        assert_eq!(first.0 + second.0, 15);
        let stats = pool.stats();
        assert_eq!(stats.in_use, 2);
        assert_eq!(stats.available, 0);
    }

    #[test]
    fn resource_pool_returns_lease_on_scope_exit() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        {
            let _lease = pool.try_borrow().expect("borrow should succeed");
            assert_eq!(pool.stats().in_use, 1);
        }
        assert_eq!(pool.stats().in_use, 0);
        assert_eq!(pool.stats().available, 1);
    }

    #[test]
    #[should_panic(expected = "RSSCRIPT_RUNTIME_DIAGNOSTIC:")]
    fn resource_pool_borrow_panics_on_exhaustion() {
        let pool = ResourcePool::<FileHandle>::empty();
        let _exhausted = pool.borrow();
    }

    #[test]
    fn resource_pool_lazy_creates_on_demand_up_to_max_size() {
        let mut next = 0;
        let pool = ResourcePool::lazy_from_factory(2, move || {
            next += 1;
            FileHandle(next)
        });
        assert_eq!(pool.stats().created, 0);

        let first = pool.try_borrow().expect("first lazy borrow should create");
        assert_eq!(first.0, 1);
        assert_eq!(pool.stats().created, 1);
        let _second = pool.try_borrow().expect("second lazy borrow should create");
        assert_eq!(pool.stats().created, 2);
        let exhausted = pool.try_borrow().expect_err("third borrow should exhaust");
        assert!(exhausted.message().contains("exhausted"));
    }

    #[test]
    fn resource_pool_discard_evicts_and_lazy_recreates() {
        let mut next = 0;
        let pool = ResourcePool::lazy_from_factory(1, move || {
            next += 1;
            FileHandle(next)
        });
        {
            let mut lease = pool.try_borrow().expect("borrow should create #1");
            assert_eq!(lease.0, 1);
            resource_lease_discard(&mut lease);
        }
        // The discarded resource was evicted, so the lazy pool recreates a fresh
        // one rather than reporting exhaustion.
        assert_eq!(pool.stats().created, 0);
        let fresh = pool.try_borrow().expect("borrow should create #2");
        assert_eq!(fresh.0, 2);
    }

    #[test]
    fn resource_pool_lazy_non_positive_max_size_reports_invalid_capacity() {
        // A misconfigured (<= 0) lazy capacity reports an explicit invalid-capacity
        // error instead of masquerading as a perpetually-exhausted pool.
        let pool = ResourcePool::lazy_from_factory(0, || FileHandle(1));
        let error = pool
            .try_borrow()
            .expect_err("zero-capacity lazy pool should be invalid");
        assert!(
            error.message().contains("must be positive"),
            "{}",
            error.message()
        );

        let negative = ResourcePool::lazy_from_factory(-3, || FileHandle(1));
        let error = negative
            .try_borrow()
            .expect_err("negative-capacity lazy pool should be invalid");
        assert!(error.message().contains("-3"), "{}", error.message());
    }

    #[test]
    fn resource_pool_try_lazy_surfaces_factory_failure() {
        let pool = ResourcePool::<FileHandle>::try_lazy_from_factory(2, || {
            Err(PoolError::factory_failed("connect refused"))
        });
        let error = pool
            .try_borrow()
            .expect_err("factory failure should surface");
        assert!(
            error.message().contains("connect refused"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn resource_pool_lazy_factory_can_inspect_stats_without_borrow_panic() {
        let holder: std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<ResourcePool<FileHandle>>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let factory_holder = std::rc::Rc::clone(&holder);
        let pool = std::rc::Rc::new(ResourcePool::lazy_from_factory(1, move || {
            let pool = factory_holder.borrow();
            let stats = pool
                .as_ref()
                .expect("pool should be installed before borrowing")
                .stats();
            assert_eq!(pool_stats_created(&stats), 0);
            FileHandle(9)
        }));
        *holder.borrow_mut() = Some(std::rc::Rc::clone(&pool));

        let lease = pool.try_borrow().expect("factory should create a resource");
        assert_eq!(lease.0, 9);
    }

    #[test]
    fn resource_pool_reentrant_factory_borrow_is_fallible() {
        let holder: std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<ResourcePool<FileHandle>>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let factory_holder = std::rc::Rc::clone(&holder);
        let pool = std::rc::Rc::new(ResourcePool::try_lazy_from_factory(1, move || {
            let pool = factory_holder.borrow();
            let error = pool
                .as_ref()
                .expect("pool should be installed before borrowing")
                .try_borrow()
                .expect_err("factory reentrancy should be rejected");
            assert!(error.message().contains("re-entered"));
            Ok(FileHandle(11))
        }));
        *holder.borrow_mut() = Some(std::rc::Rc::clone(&pool));

        let lease = pool
            .try_borrow()
            .expect("outer borrow should still succeed");
        assert_eq!(lease.0, 11);
    }

    #[test]
    #[should_panic(expected = "RSSCRIPT_RUNTIME_DIAGNOSTIC:")]
    fn assert_equal_failure_uses_runtime_diagnostic() {
        crate::assert_equal("left", "right");
    }

    #[test]
    fn assertion_failed_error_has_structured_kind() {
        let error = assertion_failed_error("assertion failed".to_string());

        assert_eq!(error.kind, RuntimeErrorKind::AssertionFailed);
        assert!(
            error
                .diagnostic_json()
                .contains("\"kind\":\"assertion_failed\"")
        );
    }

    #[test]
    fn file_runtime_hooks_write_and_read_bytes() {
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-file-{}.txt", std::process::id()));

        {
            let mut file = crate::file_open_write(&path).expect("file should open for write");
            crate::file_write_string(&mut file, "hello file").expect("write should succeed");
        }

        let mut file = crate::file_open_read(&path).expect("file should open for read");
        let bytes = crate::file_read_all(&mut file).expect("read should succeed");
        let mut file = crate::file_open_read(&path).expect("file should reopen for text read");
        let text = crate::file_read_all_string(&mut file).expect("text read should succeed");

        assert_eq!(bytes, b"hello file");
        assert_eq!(text, "hello file");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_runtime_hooks_read_into_reusable_buffer() {
        let path = std::env::temp_dir().join(format!(
            "rsscript-runtime-file-buffer-{}.txt",
            std::process::id()
        ));

        {
            let mut file = crate::file_open_write(&path).expect("file should open for write");
            crate::file_write(&mut file, &"hello buffer").expect("write should succeed");
        }

        let mut file = crate::file_open_read(&path).expect("file should open for read");
        let mut buffer = crate::buffer_new(5);
        assert!(crate::file_read_into(&mut file, &mut buffer).expect("read should succeed"));
        assert_eq!(buffer, b"hello");
        assert!(crate::file_read_into(&mut file, &mut buffer).expect("read should succeed"));
        assert_eq!(buffer, b" buff");
        assert!(crate::file_read_into(&mut file, &mut buffer).expect("read should succeed"));
        assert_eq!(buffer, b"er");
        assert!(!crate::file_read_into(&mut file, &mut buffer).expect("EOF should succeed"));
        assert!(buffer.is_empty());
        crate::buffer_clear(&mut buffer);
        assert!(buffer.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_bytes_stream_reads_chunks_lazily() {
        let path = std::env::temp_dir().join(format!(
            "rsscript-runtime-file-stream-{}.txt",
            std::process::id()
        ));

        {
            let mut file = crate::file_open_write(&path).expect("file should open for write");
            crate::file_write(&mut file, &"abcdef").expect("write should succeed");
        }

        let stream = crate::file_bytes_stream(&path, 2).expect("stream should open");
        let mut executor = crate::Executor::new();

        assert_eq!(
            executor.run_pending(crate::stream_next(&stream)).unwrap(),
            Some(b"ab".to_vec())
        );
        assert_eq!(
            crate::stream_collect_list(&stream).unwrap(),
            vec![b"cd".to_vec(), b"ef".to_vec()]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn directory_runtime_hooks_cover_package_manager_fs_checks() {
        let root =
            std::env::temp_dir().join(format!("rsscript-runtime-directory-{}", std::process::id()));
        let nested = root.join("src");
        let file = nested.join("main.rss");

        crate::directory_create_all(&nested).expect("directory should be created");
        std::fs::write(&file, "pub fn main() -> Unit {}\n").expect("file should write");

        assert!(crate::directory_exists(&root));
        assert!(crate::directory_is_dir(&root));
        assert!(!crate::directory_is_file(&root));
        assert!(crate::directory_exists(&file));
        assert!(crate::directory_is_file(&file));
        assert!(!crate::directory_is_dir(&file));
        assert_eq!(
            crate::directory_list_files(&root).expect("files should list"),
            vec!["src/main.rss".to_string()]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn json_runtime_hooks_parse_nested_fields() {
        let json_text = r#"{"profiles":[{"name":"RSScript","age":1,"active":true}]}"#;
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-json-{}.json", std::process::id()));
        std::fs::write(&path, json_text).expect("JSON fixture should write");

        let value = crate::json_parse(json_text).expect("JSON should parse");
        let value_from_file = crate::json_parse_file(&path).expect("JSON file should parse");
        let quoted = crate::json_quote_string("RSScript \"review\"");
        let profiles_from_file =
            crate::json_field(&value_from_file, "profiles").expect("profiles should exist");
        let file_len =
            crate::json_array_len(&profiles_from_file).expect("file profiles should be an array");
        let profiles = crate::json_field(&value, "profiles").expect("profiles field should exist");
        let len = crate::json_array_len(&profiles).expect("profiles should be an array");
        let profile = crate::json_array_get(&profiles, 0).expect("first profile should exist");
        let name =
            crate::json_field_string(&profile, "name").expect("name field should be a string");
        let profile_name =
            crate::json_as_string(&crate::json_field(&profile, "name").unwrap()).unwrap();
        let profile_name_value = crate::json_field(&profile, "name").unwrap();
        let age = crate::json_field_int(&profile, "age").expect("age should be an integer");
        let active = crate::json_field_bool(&profile, "active").expect("active should be a bool");
        let age_value = crate::json_field(&profile, "age").unwrap();
        let active_value = crate::json_field(&profile, "active").unwrap();
        let age_again = crate::json_as_int(&age_value).expect("age value should be an integer");
        let active_again =
            crate::json_as_bool(&active_value).expect("active value should be a boolean");
        let profile_is_object = crate::json_is_object(&profile);
        let profile_object_len = crate::json_object_len(&profile).unwrap();
        let profile_object_keys = crate::json_object_keys(&profile).unwrap();
        let profiles_is_array = crate::json_is_array(&profiles);
        let profile_name_is_null = crate::json_is_null(&profile_name_value);
        let reasons =
            crate::json_parse(r#"["public entry point","error handling boundary"]"#).unwrap();
        let reason_strings = crate::json_array_strings(&reasons).unwrap();
        let numbers = crate::json_parse("[1, 2]").unwrap();
        let number_values = crate::json_array_ints(&numbers).unwrap();
        let flags = crate::json_parse("[true, false]").unwrap();
        let flag_values = crate::json_array_bools(&flags).unwrap();
        let has_public = crate::json_array_contains_string(&reasons, "public entry point").unwrap();
        let has_native = crate::json_array_contains_string(&reasons, "native boundary").unwrap();
        let has_error = crate::json_array_contains_substring(&reasons, "error handling").unwrap();
        let has_pool = crate::json_array_contains_substring(&reasons, "ResourcePool").unwrap();
        let has_public_prefix = crate::json_array_contains_prefix(&reasons, "public").unwrap();
        let public_count = crate::json_array_count_where(&reasons, |item| {
            Ok(crate::json_as_string(&item)?.starts_with("public"))
        })
        .unwrap();
        let folded_count = crate::json_array_fold(&reasons, &0_i64, |count, item| {
            if crate::json_as_string(&item)?.contains("boundary") {
                return Ok(count + 1);
            }
            Ok(count)
        })
        .unwrap();
        let name_starts_with_rss = crate::string_starts_with(&name, "RSS");
        let name_ends_with_script = crate::string_ends_with(&name, "Script");
        let name_contains_script = crate::string_contains(&name, "Script");
        let lines = crate::string_lines("pub fn Api.run()\nreturn Unit\n");
        let joined = crate::string_join(&lines, " | ");
        let stripped = crate::string_strip_prefix("pub fn Api.run() -> Unit", "pub fn ");
        let before_args = crate::string_before("Api.run() -> Unit", "(");
        let after_return = crate::string_after("Api.run() -> Unit", "-> ");
        let empty = crate::string_is_empty("");
        let trimmed = crate::string_trim("  review  ");
        let lower = crate::string_to_lowercase("Review");
        let upper = crate::string_to_uppercase("review");
        let replaced = crate::string_replace("review map", "map", "plan");
        let split = crate::string_split("review,map", ",");
        let mut builder = crate::string_builder_new();
        crate::string_builder_push(&mut builder, "selfhost ");
        crate::string_builder_push(&mut builder, "summary");
        let built = crate::string_builder_finish(builder);

        assert_eq!(file_len, 1);
        assert_eq!(quoted, "\"RSScript \\\"review\\\"\"");
        assert_eq!(len, 1);
        assert_eq!(name, "RSScript");
        assert_eq!(profile_name, "RSScript");
        assert_eq!(age, 1);
        assert!(active);
        assert_eq!(age_again, 1);
        assert!(active_again);
        assert!(profile_is_object);
        assert_eq!(profile_object_len, 3);
        assert_eq!(
            profile_object_keys,
            vec!["active".to_string(), "age".to_string(), "name".to_string()]
        );
        assert!(profiles_is_array);
        assert!(!profile_name_is_null);
        assert_eq!(
            reason_strings,
            vec!["public entry point", "error handling boundary"]
        );
        assert_eq!(number_values, vec![1, 2]);
        assert_eq!(flag_values, vec![true, false]);
        assert!(has_public);
        assert!(!has_native);
        assert!(has_error);
        assert!(!has_pool);
        assert!(has_public_prefix);
        assert_eq!(public_count, 1);
        assert_eq!(folded_count, 1);
        assert!(name_starts_with_rss);
        assert!(name_ends_with_script);
        assert!(name_contains_script);
        assert_eq!(lines, vec!["pub fn Api.run()", "return Unit"]);
        assert_eq!(joined, "pub fn Api.run() | return Unit");
        assert_eq!(stripped.as_deref(), Some("Api.run() -> Unit"));
        assert_eq!(before_args.as_deref(), Some("Api.run"));
        assert_eq!(after_return.as_deref(), Some("Unit"));
        assert!(empty);
        assert_eq!(trimmed, "review");
        assert_eq!(lower, "review");
        assert_eq!(upper, "REVIEW");
        assert_eq!(replaced, "review plan");
        assert_eq!(split, vec!["review", "map"]);
        assert_eq!(built, "selfhost summary");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn csv_runtime_hooks_read_and_parse_row() {
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-csv-{}.csv", std::process::id()));

        {
            let mut file = crate::file_open_write(&path).expect("file should open for write");
            crate::file_write(&mut file, &"name,amount\nRSScript,42\n")
                .expect("write should succeed");
        }

        let mut file = crate::file_open_read(&path).expect("file should open for read");
        let mut buffer = crate::row_buffer_new(4096);
        crate::csv_read_into(&mut file, &mut buffer).expect("CSV read should succeed");
        let row = crate::csv_parse_row(&buffer).expect("CSV row should parse");
        let name = crate::row_field_string(&row, 0).expect("field should exist");

        assert_eq!(name, "RSScript");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn csv_rows_stream_reads_rows_lazily() {
        let path = std::env::temp_dir().join(format!(
            "rsscript-runtime-csv-stream-{}.csv",
            std::process::id()
        ));

        {
            let mut file = crate::file_open_write(&path).expect("file should open for write");
            crate::file_write(&mut file, &"name,amount\nRSScript,42\nReview,7\n")
                .expect("write should succeed");
        }

        let stream = crate::csv_rows(&path, 64).expect("CSV stream should open");
        let mut executor = crate::Executor::new();
        let row = executor
            .run_pending(crate::stream_next(&stream))
            .expect("stream next should succeed")
            .expect("first row should exist");
        assert_eq!(
            crate::row_field_string(&row, 0).expect("field should exist"),
            "RSScript"
        );
        let rows = crate::stream_collect_list(&stream).expect("collect should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            crate::row_field_string(&rows[0], 1).expect("field should exist"),
            "7"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn counter_runtime_hooks_mutate_counter() {
        let mut counter = crate::counter_new(1);

        crate::counter_add(&mut counter, 2);

        assert_eq!(crate::counter_value(&counter), 3);
    }

    #[test]
    fn consume_runtime_hooks_take_collections() {
        crate::list_consume(vec![1_i64, 2, 3]);
        crate::buffer_consume(b"bytes".to_vec());
        crate::os_close(0);
    }

    #[test]
    fn list_runtime_hooks_count_with_predicate() {
        let values = vec![1_i64, 2, 3, 4];

        let even = crate::list_count_where(&values, |value| value % 2 == 0);
        let any_gt_three = crate::list_any(&values, |value| value > 3);
        let all_positive = crate::list_all(&values, |value| value > 0);
        let found_three = crate::list_find(&values, |value| value == 3);
        let filtered = crate::list_filter(&values, |value| value > 2);
        let mapped = crate::list_map(&values, |value| value + 10);
        let sum = crate::list_fold(&values, &0_i64, |state, value| state + value);
        let try_sum = crate::list_try_fold(
            &values,
            &0_i64,
            |state, value| -> Result<i64, crate::JsonError> { Ok(state + value) },
        )
        .unwrap();

        assert_eq!(even, 2);
        assert!(any_gt_three);
        assert!(all_positive);
        assert_eq!(found_three, Some(3));
        assert_eq!(filtered, vec![3, 4]);
        assert_eq!(mapped, vec![11, 12, 13, 14]);
        assert_eq!(sum, 10);
        assert_eq!(try_sum, 10);
    }

    #[test]
    fn map_and_set_runtime_hooks_cover_common_operations() {
        let mut map = crate::map_new::<String, i64>();
        let key = "one".to_string();

        assert!(crate::map_is_empty(&map));
        crate::map_insert(&mut map, &key, &1);
        assert_eq!(crate::map_len(&map), 1);
        assert!(crate::map_contains_key(&map, &key));
        assert_eq!(crate::map_get(&map, &key), Some(1));
        assert_eq!(crate::map_remove(&mut map, &key), Some(1));
        assert!(crate::map_is_empty(&map));
        crate::map_insert(&mut map, &key, &2);
        crate::map_clear(&mut map);
        assert!(crate::map_is_empty(&map));

        let mut set = crate::set_new::<String>();

        assert!(crate::set_is_empty(&set));
        assert!(crate::set_insert(&mut set, &key));
        assert_eq!(crate::set_len(&set), 1);
        assert!(crate::set_contains(&set, &key));
        assert!(crate::set_remove(&mut set, &key));
        assert!(crate::set_is_empty(&set));
        assert!(crate::set_insert(&mut set, &key));
        crate::set_clear(&mut set);
        assert!(crate::set_is_empty(&set));
    }

    #[test]
    fn cache_runtime_hooks_insert_and_lookup_values() {
        let mut cache = crate::cache_new();

        crate::cache_insert(&mut cache, "/users", "handled /users");

        assert_eq!(crate::cache_lookup(&cache, "/users"), "handled /users");
        assert_eq!(crate::cache_get(&cache).bytes, b"handled /users");
    }

    #[test]
    fn interpreter_runtime_hooks_link_environment_function_cycle() {
        let root = crate::manage(crate::environment_root());
        let child = crate::manage(crate::environment_child(&root));
        let function = crate::manage(crate::function_object_new(&child));
        let mut child_handle = child.clone();

        crate::environment_bind_function(&mut child_handle, &function);

        assert!(crate::environment_has_parent(&child));
        assert!(crate::environment_has_function(&child));
        assert!(crate::function_object_has_closure(&function));

        drop(child);
        drop(child_handle);

        assert!(!crate::function_object_has_closure(&function));
    }

    #[test]
    fn image_runtime_hooks_load_transform_and_save() {
        let input = std::env::temp_dir().join(format!(
            "rsscript-runtime-image-input-{}.bin",
            std::process::id()
        ));
        let output = std::env::temp_dir().join(format!(
            "rsscript-runtime-image-output-{}.bin",
            std::process::id()
        ));
        std::fs::write(&input, b"image-bytes").expect("input image should be writable");

        let mut image = crate::image_load(&input).expect("image should load");
        crate::image_resize(&mut image, 320, 240);
        crate::image_normalize(&mut image);
        crate::image_sharpen(&mut image);
        crate::image_save(&image, &output).expect("image should save");

        let saved = std::fs::read_to_string(&output).expect("saved image should be readable");
        assert!(saved.contains("rsscript-image-ops:load,resize,normalize,sharpen;size=320x240"));

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn http_runtime_hooks_create_request_and_response() {
        let request = crate::request_new("/users");
        let path = crate::request_path(&request);
        let response =
            crate::response_ok(&format!("handled {path}")).expect("response should build");

        assert_eq!(path, "/users");
        assert_eq!(crate::response_status(&response), 200);
        assert_eq!(crate::response_body(&response), "handled /users");
        assert_eq!(crate::http_response_status(&response), 200);
        assert_eq!(crate::http_response_text(&response), "handled /users");
        assert!(crate::http_response_is_success(&response));

        // Sync `http_get` performs a real request (see `http_get_sync_*` tests); with
        // no reachable host the request fails, surfacing the target URL in the error.
        let error = crate::http_get("https://example.test")
            .expect_err("unreachable host should fail the request");
        assert!(
            crate::http_error_message(&error).contains("https://example.test"),
            "error should name the failed request target: {}",
            crate::http_error_message(&error)
        );
    }

    #[test]
    fn timer_sleep_pending_completes() {
        crate::run_pending(crate::timer_sleep_start(0)).expect("timer sleep should complete");
    }

    #[test]
    fn executor_drives_pending_with_context() {
        struct CountPending {
            polls_before_ready: usize,
        }

        impl crate::Pending<usize> for CountPending {
            fn poll(&mut self, cx: &mut crate::Context<'_>) -> crate::AsyncPoll<usize> {
                if self.polls_before_ready == 0 {
                    return crate::AsyncPoll::Ready(42);
                }
                self.polls_before_ready -= 1;
                cx.yield_now();
                crate::AsyncPoll::Pending
            }
        }

        let mut executor = crate::Executor::new();
        let value = executor.run_pending(CountPending {
            polls_before_ready: 2,
        });

        assert_eq!(value, 42);
        assert_eq!(executor.poll_count(), 3);
        assert!(executor.yield_count() >= 2);
    }

    #[test]
    fn task_group_joins_pending_tasks_in_spawn_order() {
        struct CountPending {
            value: usize,
            polls_before_ready: usize,
        }

        impl crate::Pending<usize> for CountPending {
            fn poll(&mut self, cx: &mut crate::Context<'_>) -> crate::AsyncPoll<usize> {
                if self.polls_before_ready == 0 {
                    return crate::AsyncPoll::Ready(self.value);
                }
                self.polls_before_ready -= 1;
                cx.yield_now();
                crate::AsyncPoll::Pending
            }
        }

        let mut group = crate::TaskGroup::new();
        group.spawn_pending(CountPending {
            value: 10,
            polls_before_ready: 2,
        });
        group.spawn_pending(CountPending {
            value: 20,
            polls_before_ready: 0,
        });

        let mut executor = crate::Executor::new();
        let outputs = executor.run_task_group(group);

        assert_eq!(outputs, vec![10, 20]);
        assert!(executor.poll_count() >= 3);
        assert!(executor.yield_count() >= 2);
    }

    #[test]
    fn task_group_join_until_times_out_with_completed_outputs() {
        struct MaybePending {
            value: usize,
            ready: bool,
        }

        impl crate::Pending<usize> for MaybePending {
            fn poll(&mut self, _cx: &mut crate::Context<'_>) -> crate::AsyncPoll<usize> {
                if self.ready {
                    crate::AsyncPoll::Ready(self.value)
                } else {
                    crate::AsyncPoll::Pending
                }
            }
        }

        let mut group = crate::TaskGroup::new();
        group.spawn_pending(MaybePending {
            value: 10,
            ready: true,
        });
        group.spawn_pending(MaybePending {
            value: 20,
            ready: false,
        });

        let mut executor = crate::Executor::new();
        let outcome = group.join_until(
            &mut executor,
            std::time::Instant::now() + std::time::Duration::from_millis(1),
        );

        assert!(matches!(
            outcome,
            crate::TaskGroupJoin::TimedOut {
                completed,
                pending: 1
            } if completed == vec![Some(10), None]
        ));
    }

    #[test]
    fn task_group_join_until_cancels_pending_tasks_on_timeout() {
        struct NeverReady;

        impl crate::Pending<usize> for NeverReady {
            fn poll(&mut self, _cx: &mut crate::Context<'_>) -> crate::AsyncPoll<usize> {
                crate::AsyncPoll::Pending
            }
        }

        let mut group = crate::TaskGroup::new();
        let cancellation = group.cancellation_token();
        group.spawn_pending(NeverReady);

        let mut executor = crate::Executor::new();
        let outcome = group.join_until(
            &mut executor,
            std::time::Instant::now() + std::time::Duration::from_millis(1),
        );

        assert!(matches!(
            outcome,
            crate::TaskGroupJoin::TimedOut {
                completed,
                pending: 1
            } if completed == vec![None]
        ));
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn task_group_spawn_cancellable_passes_group_token_to_pending() {
        struct CapturesCancellation {
            token: crate::CancellationToken,
        }

        impl crate::Pending<bool> for CapturesCancellation {
            fn poll(&mut self, _cx: &mut crate::Context<'_>) -> crate::AsyncPoll<bool> {
                crate::AsyncPoll::Ready(self.token.is_cancelled())
            }
        }

        let mut group = crate::TaskGroup::new();
        group.cancel();
        group.spawn_cancellable(|token| Box::new(CapturesCancellation { token }));

        let mut executor = crate::Executor::new();
        let outputs = executor.run_task_group(group);

        assert_eq!(outputs, vec![true]);
    }

    #[test]
    fn task_group_join_until_wakes_from_native_signal() {
        struct ExternalWakePending {
            ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
            started: bool,
        }

        impl crate::Pending<usize> for ExternalWakePending {
            fn poll(&mut self, cx: &mut crate::Context<'_>) -> crate::AsyncPoll<usize> {
                if self.ready.load(std::sync::atomic::Ordering::SeqCst) {
                    return crate::AsyncPoll::Ready(42);
                }
                if !self.started {
                    self.started = true;
                    let ready = self.ready.clone();
                    let wake = cx.wake_handle();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        ready.store(true, std::sync::atomic::Ordering::SeqCst);
                        wake.wake();
                    });
                }
                crate::AsyncPoll::Pending
            }
        }

        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut group = crate::TaskGroup::new();
        group.spawn_pending(ExternalWakePending {
            ready,
            started: false,
        });

        let mut executor = crate::Executor::new();
        let started = std::time::Instant::now();
        let outcome = group.join_until(&mut executor, started + std::time::Duration::from_secs(1));

        assert!(matches!(outcome, crate::TaskGroupJoin::Completed(values) if values == vec![42]));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn cancellable_pending_ignores_late_native_wake_after_timeout() {
        struct LateNativePending {
            ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
            token: crate::CancellationToken,
            started: bool,
        }

        impl crate::Pending<usize> for LateNativePending {
            fn poll(&mut self, cx: &mut crate::Context<'_>) -> crate::AsyncPoll<usize> {
                if self.token.is_cancelled() {
                    return crate::AsyncPoll::Pending;
                }
                if self.ready.load(std::sync::atomic::Ordering::SeqCst) {
                    return crate::AsyncPoll::Ready(7);
                }
                if !self.started {
                    self.started = true;
                    let ready = self.ready.clone();
                    let wake = cx.wake_handle();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        ready.store(true, std::sync::atomic::Ordering::SeqCst);
                        wake.wake();
                    });
                }
                crate::AsyncPoll::Pending
            }
        }

        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut group = crate::TaskGroup::new();
        let cancellation = group.cancellation_token();
        let pending_ready = ready.clone();
        group.spawn_cancellable(|token| {
            Box::new(LateNativePending {
                ready: pending_ready,
                token,
                started: false,
            })
        });

        let mut executor = crate::Executor::new();
        let outcome = group.join_until(
            &mut executor,
            std::time::Instant::now() + std::time::Duration::from_millis(1),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));

        assert!(matches!(
            outcome,
            crate::TaskGroupJoin::TimedOut {
                completed,
                pending: 1
            } if completed == vec![None]
        ));
        assert!(cancellation.is_cancelled());
        assert!(ready.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn native_async_pending_completes_from_background_worker() {
        let cancellation = crate::CancellationToken::new();
        let (pending, completer) = crate::native_async_pending(cancellation);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert!(completer.complete(99));
        });

        let mut executor = crate::Executor::new();
        let started = std::time::Instant::now();
        let value = executor.run_pending(pending);

        assert_eq!(value, 99);
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn native_timer_sleep_pending_completes_through_completion_slot() {
        let pending = crate::timer_sleep_native_start(5);

        let mut executor = crate::Executor::new();
        let started = std::time::Instant::now();
        let value = executor.run_pending(pending);

        assert_eq!(value, Ok(()));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn native_async_completer_can_finish_after_cancellation() {
        let cancellation = crate::CancellationToken::new();
        let (_pending, completer) = crate::native_async_pending::<usize>(cancellation.clone());

        cancellation.cancel();

        assert!(completer.complete(99));
    }

    #[test]
    fn process_runtime_hook_captures_stdout_and_failure() {
        let args = vec!["--version".to_string()];
        let stdout = crate::process_run_stdout("cargo", &args).expect("cargo should run");
        assert!(stdout.contains("cargo"), "{stdout}");

        let error = crate::process_run_stdout("__rsscript_missing_process__", &[])
            .expect_err("missing command should fail");
        assert!(error.contains("failed to run `__rsscript_missing_process__`"));
    }

    #[test]
    fn process_runtime_hook_runs_many_commands() {
        let args = Vec::<String>::new();
        let appended_args = vec!["--version".to_string(), "--version".to_string()];
        let stdout = crate::process_run_many_stdout("cargo", &args, &appended_args, 2)
            .expect("cargo should run");

        assert_eq!(stdout.len(), 2);
        assert!(stdout.iter().all(|output| output.contains("cargo")));
    }

    #[test]
    fn db_runtime_hooks_pool_connections() {
        let pool = ResourcePool::from_factory(2, || crate::db_connection_open("db://local"));
        {
            let mut conn = pool.borrow();
            crate::db_connection_query(&mut conn, "select 1").expect("query should run");
        }

        assert_eq!(pool.stats().created, 2);
    }

    #[test]
    fn config_runtime_hooks_load_and_replace_store() {
        let first = std::env::temp_dir().join(format!(
            "rsscript-runtime-config-first-{}.txt",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "rsscript-runtime-config-second-{}.txt",
            std::process::id()
        ));
        std::fs::write(&first, "initial\n").expect("first config should write");
        std::fs::write(&second, "reloaded\n").expect("second config should write");

        let initial = crate::config_load(&first).expect("initial config should load");
        let mut store = crate::config_store_new(&initial);
        let reloaded = crate::config_load(&second).expect("reloaded config should load");
        crate::config_store_replace(&mut store, &reloaded);

        assert_eq!(crate::config_store_name(&store), "reloaded");

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn rules_config_runtime_hooks_load_and_replace_global() {
        let first = std::env::temp_dir().join(format!(
            "rsscript-runtime-rules-first-{}.txt",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "rsscript-runtime-rules-second-{}.txt",
            std::process::id()
        ));
        std::fs::write(&first, "alpha\nbeta\n").expect("first rules should write");
        std::fs::write(&second, "gamma\n").expect("second rules should write");

        let first_rules = crate::rule_loader_load_rules(&first).expect("first rules should load");
        let initial = crate::config_new("initial", &first_rules);
        let mut global = crate::global_config_new(&initial);
        let second_rules =
            crate::rule_loader_load_rules(&second).expect("second rules should load");
        let next = crate::config_new("next", &second_rules);
        crate::global_config_replace(&mut global, &next);

        assert_eq!(crate::global_config_rule_count(&global), 1);

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }
}
