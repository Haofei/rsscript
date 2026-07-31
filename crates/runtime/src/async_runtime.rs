use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::domain::TimerError;
use tracing::info;

pub enum AsyncPoll<T> {
    Ready(T),
    Pending,
}

#[derive(Debug, Default)]
pub struct Executor {
    polls: usize,
    yields: usize,
    next_wake: Option<Instant>,
    wake_signal: WakeHandle,
}

impl Executor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn poll_count(&self) -> usize {
        self.polls
    }

    pub fn yield_count(&self) -> usize {
        self.yields
    }

    pub fn run_pending<T>(&mut self, mut pending: impl Pending<T>) -> T {
        let started = Instant::now();
        let start_polls = self.polls;
        let start_yields = self.yields;
        loop {
            self.polls += 1;
            let poll = {
                let mut cx = Context {
                    executor: self,
                    wake_key: None,
                };
                pending.poll(&mut cx)
            };
            match poll {
                AsyncPoll::Ready(value) => {
                    info!(
                        phase = "executor_run_pending",
                        elapsed_us = started.elapsed().as_micros(),
                        polls = self.polls.saturating_sub(start_polls),
                        yields = self.yields.saturating_sub(start_yields),
                        "async_runtime_phase"
                    );
                    return value;
                }
                AsyncPoll::Pending => {
                    self.park_or_yield(None);
                }
            }
        }
    }

    pub fn poll_once<T>(&mut self, pending: &mut impl Pending<T>) -> AsyncPoll<T> {
        self.polls += 1;
        let mut cx = Context {
            executor: self,
            wake_key: None,
        };
        pending.poll(&mut cx)
    }

    pub fn poll_once_keyed<T>(
        &mut self,
        wake_key: usize,
        pending: &mut impl Pending<T>,
    ) -> AsyncPoll<T> {
        self.polls += 1;
        let mut cx = Context {
            executor: self,
            wake_key: Some(wake_key),
        };
        pending.poll(&mut cx)
    }

    pub fn yield_once(&mut self) {
        self.yields += 1;
        std::thread::yield_now();
    }

    pub fn wait_for_wake(&mut self) {
        self.park_or_yield(None);
    }

    pub fn drain_ready_wake_keys(&mut self) -> Vec<usize> {
        self.wake_signal.drain_ready_keys()
    }

    pub fn run_task_group<T>(&mut self, group: TaskGroup<T>) -> Vec<T> {
        group.join(self)
    }

    pub fn wake_handle(&self) -> WakeHandle {
        self.wake_signal.clone()
    }

    fn request_wake_at(&mut self, deadline: Instant) {
        self.next_wake = Some(
            self.next_wake
                .map_or(deadline, |current| current.min(deadline)),
        );
    }

    fn park_or_yield(&mut self, limit: Option<Instant>) {
        self.yields += 1;
        let next_wake = self.next_wake.take();
        let target = match (next_wake, limit) {
            (Some(wake), Some(limit)) => Some(wake.min(limit)),
            (Some(wake), None) => Some(wake),
            (None, Some(limit)) => Some(limit),
            (None, None) => None,
        };
        let Some(deadline) = target else {
            self.wake_signal.wait();
            return;
        };

        let now = Instant::now();
        if deadline > now {
            self.wake_signal
                .wait_timeout(deadline.saturating_duration_since(now));
            return;
        }
        std::thread::yield_now();
    }
}

#[derive(Debug, Clone, Default)]
pub struct WakeHandle {
    signal: Arc<WakeSignal>,
    key: Option<usize>,
}

#[derive(Debug, Default)]
struct WakeSignal {
    state: Mutex<WakeState>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct WakeState {
    woken: bool,
    ready_keys: Vec<usize>,
    ready_key_set: HashSet<usize>,
}

impl WakeHandle {
    pub fn wake(&self) {
        let mut state = self.signal.state.lock().expect("wake signal lock poisoned");
        state.woken = true;
        if let Some(key) = self.key
            && state.ready_key_set.insert(key)
        {
            state.ready_keys.push(key);
        }
        self.signal.ready.notify_all();
    }

    fn wait_timeout(&self, duration: Duration) {
        let mut state = self.signal.state.lock().expect("wake signal lock poisoned");
        if state.woken {
            state.woken = false;
            return;
        }
        let (mut state_after_wait, _) = self
            .signal
            .ready
            .wait_timeout(state, duration)
            .expect("wake signal condvar wait poisoned");
        if state_after_wait.woken {
            state_after_wait.woken = false;
        }
    }

    fn wait(&self) {
        let mut state = self.signal.state.lock().expect("wake signal lock poisoned");
        while !state.woken {
            state = self
                .signal
                .ready
                .wait(state)
                .expect("wake signal condvar wait poisoned");
        }
        state.woken = false;
    }

    fn with_key(&self, key: Option<usize>) -> Self {
        Self {
            signal: self.signal.clone(),
            key,
        }
    }

    fn drain_ready_keys(&self) -> Vec<usize> {
        let mut state = self.signal.state.lock().expect("wake signal lock poisoned");
        state.ready_key_set.clear();
        std::mem::take(&mut state.ready_keys)
    }
}

#[derive(Debug, Clone)]
struct WeakWakeHandle {
    signal: Weak<WakeSignal>,
    key: Option<usize>,
}

impl WeakWakeHandle {
    fn new(wake: &WakeHandle) -> Self {
        Self {
            signal: Arc::downgrade(&wake.signal),
            key: wake.key,
        }
    }

    fn same_registration(&self, wake: &WakeHandle) -> bool {
        self.signal.ptr_eq(&Arc::downgrade(&wake.signal)) && self.key == wake.key
    }

    fn wake(self) {
        if let Some(signal) = self.signal.upgrade() {
            WakeHandle {
                signal,
                key: self.key,
            }
            .wake();
        }
    }
}

pub struct Context<'executor> {
    executor: &'executor mut Executor,
    wake_key: Option<usize>,
}

impl Context<'_> {
    pub fn yield_now(&mut self) {
        self.executor.yields += 1;
        self.executor.wake_signal.wake();
    }

    pub fn sleep_for(&mut self, duration: Duration) {
        self.wake_after(duration);
    }

    pub fn wake_at(&mut self, deadline: Instant) {
        self.executor.request_wake_at(deadline);
    }

    pub fn wake_after(&mut self, duration: Duration) {
        self.wake_at(Instant::now() + duration);
    }

    pub fn wake_handle(&self) -> WakeHandle {
        self.executor.wake_handle().with_key(self.wake_key)
    }
}

pub trait Pending<T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<T>;
}

impl<T, P> Pending<T> for &mut P
where
    P: Pending<T> + ?Sized,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<T> {
        (**self).poll(cx)
    }
}

pub fn run_pending<T>(pending: impl Pending<T>) -> T {
    Executor::new().run_pending(pending)
}

// --- Async combinators --------------------------------------------------------
//
// RSScript `async fn`s lower to compositions of these instead of running an
// executor inline. A linear async body `await op()?; ... ; return e` becomes a
// chain of `pending_try`/`pending_ready`, yielding a single `impl Pending<T>`
// the caller can register and poll cooperatively (so siblings interleave).

/// A pending that is immediately ready with `value`.
pub struct ReadyPending<T> {
    value: Option<T>,
}

pub fn pending_ready<T>(value: T) -> ReadyPending<T> {
    ReadyPending { value: Some(value) }
}

impl<T> Pending<T> for ReadyPending<T> {
    fn poll(&mut self, _cx: &mut Context<'_>) -> AsyncPoll<T> {
        AsyncPoll::Ready(
            self.value
                .take()
                .expect("ready pending polled after completion"),
        )
    }
}

/// A pending that runs a synchronous boundary exactly once, when first polled.
///
/// RSScript uses this for structured orchestration blocks that already own an
/// internal cooperative executor boundary (for example a `task_group` scope)
/// while still presenting the containing `async fn` as a `Pending` value to its
/// caller.
pub struct DeferredPending<F> {
    run: Option<F>,
}

pub fn pending_defer<T, F>(run: F) -> DeferredPending<F>
where
    F: FnOnce() -> T,
{
    DeferredPending { run: Some(run) }
}

impl<T, F> Pending<T> for DeferredPending<F>
where
    F: FnOnce() -> T,
{
    fn poll(&mut self, _cx: &mut Context<'_>) -> AsyncPoll<T> {
        let run = self
            .run
            .take()
            .expect("deferred pending polled after completion");
        AsyncPoll::Ready(run())
    }
}

enum ThenState<P, F, Q> {
    First { pending: P, then: Option<F> },
    Second { pending: Q },
    Done,
}

/// Drives `pending`, then runs `then(value)` to produce the next pending. The
/// non-`?` continuation (the awaited value, including a whole `Result`, is bound
/// without short-circuiting).
pub struct ThenPending<P, F, Q, T, U> {
    state: ThenState<P, F, Q>,
    #[allow(clippy::type_complexity)]
    marker: std::marker::PhantomData<fn() -> (T, U)>,
}

pub fn pending_then<T, U, P, F, Q>(pending: P, then: F) -> ThenPending<P, F, Q, T, U>
where
    P: Pending<T>,
    F: FnOnce(T) -> Q,
    Q: Pending<U>,
{
    ThenPending {
        state: ThenState::First {
            pending,
            then: Some(then),
        },
        marker: std::marker::PhantomData,
    }
}

impl<T, U, P, F, Q> Pending<U> for ThenPending<P, F, Q, T, U>
where
    P: Pending<T>,
    F: FnOnce(T) -> Q,
    Q: Pending<U>,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<U> {
        loop {
            match &mut self.state {
                ThenState::First { pending, then } => match pending.poll(cx) {
                    AsyncPoll::Ready(value) => {
                        let then = then.take().expect("then continuation already taken");
                        self.state = ThenState::Second {
                            pending: then(value),
                        };
                    }
                    AsyncPoll::Pending => return AsyncPoll::Pending,
                },
                ThenState::Second { pending } => match pending.poll(cx) {
                    AsyncPoll::Ready(value) => {
                        self.state = ThenState::Done;
                        return AsyncPoll::Ready(value);
                    }
                    AsyncPoll::Pending => return AsyncPoll::Pending,
                },
                ThenState::Done => panic!("then pending polled after completion"),
            }
        }
    }
}

enum TryState<P, F, Q> {
    First { pending: P, then: Option<F> },
    Second { pending: Q },
    Done,
}

/// Drives `pending` (a `Result`-yielding pending) and, on `Ok(value)`, runs the
/// continuation `then(value)` to produce the next pending — the `?`-style
/// continuation. On `Err(e)` it short-circuits with `Err(e)`. `then` is `FnOnce`
/// (it moves the captured locals of the async continuation) and is stored in an
/// `Option`, taken once when the first pending completes. The phantom carries
/// the success/error types that are otherwise only in the bounds.
pub struct TryPending<P, F, Q, T, E, U> {
    state: TryState<P, F, Q>,
    #[allow(clippy::type_complexity)]
    marker: std::marker::PhantomData<fn() -> (T, E, U)>,
}

pub fn pending_try<T, E, U, P, F, Q>(pending: P, then: F) -> TryPending<P, F, Q, T, E, U>
where
    P: Pending<Result<T, E>>,
    F: FnOnce(T) -> Q,
    Q: Pending<Result<U, E>>,
{
    TryPending {
        state: TryState::First {
            pending,
            then: Some(then),
        },
        marker: std::marker::PhantomData,
    }
}

impl<T, E, U, P, F, Q> Pending<Result<U, E>> for TryPending<P, F, Q, T, E, U>
where
    P: Pending<Result<T, E>>,
    F: FnOnce(T) -> Q,
    Q: Pending<Result<U, E>>,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<Result<U, E>> {
        loop {
            match &mut self.state {
                TryState::First { pending, then } => match pending.poll(cx) {
                    AsyncPoll::Ready(Ok(value)) => {
                        let then = then.take().expect("try continuation already taken");
                        self.state = TryState::Second {
                            pending: then(value),
                        };
                    }
                    AsyncPoll::Ready(Err(error)) => {
                        self.state = TryState::Done;
                        return AsyncPoll::Ready(Err(error));
                    }
                    AsyncPoll::Pending => return AsyncPoll::Pending,
                },
                TryState::Second { pending } => match pending.poll(cx) {
                    AsyncPoll::Ready(result) => {
                        self.state = TryState::Done;
                        return AsyncPoll::Ready(result);
                    }
                    AsyncPoll::Pending => return AsyncPoll::Pending,
                },
                TryState::Done => panic!("try pending polled after completion"),
            }
        }
    }
}

pub enum LoopControl {
    Continue,
    Break,
}

enum LoopResultState<'a, E> {
    Start,
    Body(Box<dyn Pending<Result<LoopControl, E>> + 'a>),
    Done,
}

pub struct LoopResultPending<'a, F, E> {
    body: F,
    state: LoopResultState<'a, E>,
}

pub fn pending_loop_result<'a, E, F>(body: F) -> LoopResultPending<'a, F, E>
where
    F: FnMut() -> Box<dyn Pending<Result<LoopControl, E>> + 'a>,
{
    LoopResultPending {
        body,
        state: LoopResultState::Start,
    }
}

pub struct PollFnPending<F> {
    poll: F,
}

pub fn pending_poll_fn<T, F>(poll: F) -> PollFnPending<F>
where
    F: FnMut(&mut Context<'_>) -> AsyncPoll<T>,
{
    PollFnPending { poll }
}

impl<T, F> Pending<T> for PollFnPending<F>
where
    F: FnMut(&mut Context<'_>) -> AsyncPoll<T>,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<T> {
        (self.poll)(cx)
    }
}

impl<'a, E, F> Pending<Result<(), E>> for LoopResultPending<'a, F, E>
where
    F: FnMut() -> Box<dyn Pending<Result<LoopControl, E>> + 'a>,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<Result<(), E>> {
        loop {
            match &mut self.state {
                LoopResultState::Start => {
                    self.state = LoopResultState::Body((self.body)());
                }
                LoopResultState::Body(pending) => match pending.poll(cx) {
                    AsyncPoll::Ready(Ok(LoopControl::Continue)) => {
                        self.state = LoopResultState::Start;
                    }
                    AsyncPoll::Ready(Ok(LoopControl::Break)) => {
                        self.state = LoopResultState::Done;
                        return AsyncPoll::Ready(Ok(()));
                    }
                    AsyncPoll::Ready(Err(error)) => {
                        self.state = LoopResultState::Done;
                        return AsyncPoll::Ready(Err(error));
                    }
                    AsyncPoll::Pending => return AsyncPoll::Pending,
                },
                LoopResultState::Done => panic!("loop pending polled after completion"),
            }
        }
    }
}

pub struct NativeAsyncPending<T> {
    result: Arc<Mutex<Option<T>>>,
    wake: Arc<Mutex<Option<WakeHandle>>>,
    abort_handle: Option<tokio::task::AbortHandle>,
    cancellation_registration: Arc<Mutex<Option<AbortRegistration>>>,
    _runtime_owner: Option<Arc<RuntimeServices>>,
}

#[derive(Clone)]
pub struct NativeAsyncCompleter<T> {
    result: Arc<Mutex<Option<T>>>,
    wake: Arc<Mutex<Option<WakeHandle>>>,
    cancellation_registration: Arc<Mutex<Option<AbortRegistration>>>,
}

pub fn native_async_pending<T>(
    _cancellation: CancellationToken,
) -> (NativeAsyncPending<T>, NativeAsyncCompleter<T>) {
    let result = Arc::new(Mutex::new(None));
    let wake = Arc::new(Mutex::new(None));
    let cancellation_registration = Arc::new(Mutex::new(None));
    (
        NativeAsyncPending {
            result: result.clone(),
            wake: wake.clone(),
            abort_handle: None,
            cancellation_registration: cancellation_registration.clone(),
            _runtime_owner: None,
        },
        NativeAsyncCompleter {
            result,
            wake,
            cancellation_registration,
        },
    )
}

const DEFAULT_MAX_TOKIO_WORKERS: usize = 32;
const TOKIO_WORKER_THREADS_ENV: &str = "RSSCRIPT_TOKIO_WORKER_THREADS";

fn bounded_worker_threads(configured: Option<&str>, available: usize) -> usize {
    configured
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(available)
        .clamp(1, DEFAULT_MAX_TOKIO_WORKERS)
}

pub fn tokio_native_runtime_worker_threads() -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    bounded_worker_threads(
        std::env::var(TOKIO_WORKER_THREADS_ENV).ok().as_deref(),
        available,
    )
}

struct OwnedTokioRuntime {
    handle: tokio::runtime::Handle,
    shutdown: Mutex<Option<std::sync::mpsc::SyncSender<Duration>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    closed: AtomicBool,
}

impl OwnedTokioRuntime {
    fn new(worker_threads: usize) -> Result<Self, String> {
        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
        let (shutdown_sender, shutdown_receiver) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("rsscript-runtime-owner".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(worker_threads)
                    .thread_name("rsscript-runtime-tokio")
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        if ready_sender.send(Ok(runtime.handle().clone())).is_ok() {
                            let timeout = shutdown_receiver.recv().unwrap_or_default();
                            runtime.shutdown_timeout(timeout);
                        }
                    }
                    Err(error) => {
                        let _ = ready_sender
                            .send(Err(format!("rsscript tokio runtime should start: {error}")));
                    }
                }
            })
            .map_err(|error| format!("failed to start runtime owner thread: {error}"))?;
        let handle = ready_receiver
            .recv()
            .map_err(|_| "runtime owner stopped during startup".to_string())??;
        Ok(Self {
            handle,
            shutdown: Mutex::new(Some(shutdown_sender)),
            thread: Mutex::new(Some(thread)),
            closed: AtomicBool::new(false),
        })
    }

    fn handle(&self) -> Result<tokio::runtime::Handle, String> {
        if self.closed.load(Ordering::Acquire) {
            Err("runtime services are shut down".to_string())
        } else {
            Ok(self.handle.clone())
        }
    }

    fn shutdown(&self, timeout: Duration) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(sender) = self
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(timeout);
        }
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

struct ProcessConcurrency {
    active: Mutex<usize>,
    ready: Condvar,
    limit: usize,
    closed: AtomicBool,
}

pub(crate) struct ProcessPermit {
    concurrency: Arc<ProcessConcurrency>,
    _runtime_owner: Arc<RuntimeServices>,
}

impl Drop for ProcessPermit {
    fn drop(&mut self) {
        let mut active = self
            .concurrency
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        self.concurrency.ready.notify_one();
    }
}

impl ProcessConcurrency {
    fn new(limit: usize) -> Self {
        Self {
            active: Mutex::new(0),
            ready: Condvar::new(),
            limit: limit.clamp(1, 32),
            closed: AtomicBool::new(false),
        }
    }

    fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.ready.notify_all();
    }

    fn acquire(
        self: &Arc<Self>,
        runtime_owner: Arc<RuntimeServices>,
        cancellation: Option<&RssCancellationToken>,
    ) -> Result<ProcessPermit, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "process concurrency lock poisoned".to_string())?;
        while *active >= self.limit {
            if self.closed.load(Ordering::Acquire) {
                return Err("runtime services are shut down".to_string());
            }
            if cancellation.is_some_and(cancellation_token_is_cancelled) {
                return Err("process cancelled while waiting for a concurrency slot".to_string());
            }
            let (next, _) = self
                .ready
                .wait_timeout(active, Duration::from_millis(25))
                .map_err(|_| "process concurrency lock poisoned".to_string())?;
            active = next;
        }
        if self.closed.load(Ordering::Acquire) {
            return Err("runtime services are shut down".to_string());
        }
        *active += 1;
        Ok(ProcessPermit {
            concurrency: Arc::clone(self),
            _runtime_owner: runtime_owner,
        })
    }
}

pub struct RuntimeServices {
    runtime: OwnedTokioRuntime,
    worker_threads: usize,
    process_concurrency: Arc<ProcessConcurrency>,
    #[cfg(feature = "net")]
    http_client: reqwest::Client,
}

impl RuntimeServices {
    pub fn new() -> Result<Self, String> {
        let worker_threads = tokio_native_runtime_worker_threads();
        Self::with_worker_threads(worker_threads)
    }

    pub fn with_worker_threads(worker_threads: usize) -> Result<Self, String> {
        if !(1..=DEFAULT_MAX_TOKIO_WORKERS).contains(&worker_threads) {
            return Err(format!(
                "runtime worker threads must be between 1 and {DEFAULT_MAX_TOKIO_WORKERS}"
            ));
        }
        let runtime = OwnedTokioRuntime::new(worker_threads)?;
        #[cfg(feature = "net")]
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("failed to build runtime HTTP client: {error}"))?;
        Ok(Self {
            runtime,
            worker_threads,
            process_concurrency: Arc::new(ProcessConcurrency::new(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(4),
            )),
            #[cfg(feature = "net")]
            http_client,
        })
    }

    pub fn shutdown(&self, timeout: Duration) {
        self.process_concurrency.shutdown();
        self.runtime.shutdown(timeout);
    }

    pub fn is_shutdown(&self) -> bool {
        self.runtime.closed.load(Ordering::Acquire)
    }

    pub fn worker_threads(&self) -> usize {
        self.worker_threads
    }

    pub(crate) fn runtime_handle(&self) -> Result<tokio::runtime::Handle, String> {
        self.runtime.handle()
    }

    pub(crate) fn acquire_process_permit(
        self: &Arc<Self>,
        cancellation: Option<&RssCancellationToken>,
    ) -> Result<ProcessPermit, String> {
        self.process_concurrency
            .acquire(Arc::clone(self), cancellation)
    }

    #[cfg(feature = "net")]
    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
}

impl Drop for RuntimeServices {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(1));
    }
}

pub fn spawn_tokio_native<T, F>(future: F) -> NativeAsyncPending<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    spawn_tokio_native_with_cancellation(CancellationToken::new(), future)
}

pub fn spawn_tokio_native_with_cancellation<T, F>(
    cancellation: CancellationToken,
    future: F,
) -> NativeAsyncPending<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let services = crate::compatibility::runtime_services();
    spawn_tokio_native_with_services(&services, cancellation, future)
        .expect("default runtime services should be running")
}

pub fn spawn_tokio_native_with_services<T, F>(
    services: &Arc<RuntimeServices>,
    cancellation: CancellationToken,
    future: F,
) -> Result<NativeAsyncPending<T>, String>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let (mut pending, completer) = native_async_pending(cancellation.clone());
    let (start_sender, start_receiver) = tokio::sync::oneshot::channel();
    let task = services.runtime_handle()?.spawn(async move {
        let _ = start_receiver.await;
        let value = future.await;
        completer.complete(value);
    });
    let abort_handle = task.abort_handle();
    let registration = cancellation.register_abort(abort_handle.clone());
    *pending
        .cancellation_registration
        .lock()
        .expect("cancellation registration lock poisoned") = registration;
    pending.abort_handle = Some(abort_handle);
    pending._runtime_owner = Some(Arc::clone(services));
    let _ = start_sender.send(());
    Ok(pending)
}

impl<T> NativeAsyncCompleter<T> {
    pub fn complete(&self, value: T) -> bool {
        let mut result = self
            .result
            .lock()
            .expect("native async result lock poisoned");
        if result.is_some() {
            return false;
        }
        *result = Some(value);
        drop(result);
        if let Some(wake) = self
            .wake
            .lock()
            .expect("native async wake lock poisoned")
            .as_ref()
        {
            wake.wake();
        }
        self.cancellation_registration
            .lock()
            .expect("cancellation registration lock poisoned")
            .take();
        true
    }
}

impl<T> Pending<T> for NativeAsyncPending<T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<T> {
        let mut result = self
            .result
            .lock()
            .expect("native async result lock poisoned");
        if let Some(value) = result.take() {
            return AsyncPoll::Ready(value);
        }
        *self.wake.lock().expect("native async wake lock poisoned") = Some(cx.wake_handle());
        AsyncPoll::Pending
    }
}

impl<T> Drop for NativeAsyncPending<T> {
    fn drop(&mut self) {
        self.cancellation_registration
            .lock()
            .expect("cancellation registration lock poisoned")
            .take();
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
    }
}

pub struct TaskGroup<T> {
    tasks: Vec<Box<dyn Pending<T>>>,
    cancellation: CancellationToken,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    waiters: Arc<Mutex<Vec<WeakWakeHandle>>>,
    abort_handles: Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>>,
    next_abort_id: Arc<AtomicUsize>,
    notification: Arc<tokio::sync::Notify>,
}

struct AbortRegistration {
    abort_handles: Weak<Mutex<HashMap<usize, tokio::task::AbortHandle>>>,
    id: usize,
}

impl Drop for AbortRegistration {
    fn drop(&mut self) {
        if let Some(abort_handles) = self.abort_handles.upgrade() {
            abort_handles
                .lock()
                .expect("cancellation abort handle lock poisoned")
                .remove(&self.id);
        }
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        let waiters = std::mem::take(
            &mut *self
                .waiters
                .lock()
                .expect("cancellation waiter lock poisoned"),
        );
        for waiter in waiters {
            waiter.wake();
        }
        let abort_handles = std::mem::take(
            &mut *self
                .abort_handles
                .lock()
                .expect("cancellation abort handle lock poisoned"),
        );
        for abort_handle in abort_handles.into_values() {
            abort_handle.abort();
        }
        self.notification.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn register_wake(&self, wake: WakeHandle) {
        if self.is_cancelled() {
            wake.wake();
            return;
        }
        let mut waiters = self
            .waiters
            .lock()
            .expect("cancellation waiter lock poisoned");
        if self.is_cancelled() {
            drop(waiters);
            wake.wake();
            return;
        }
        if !waiters
            .iter()
            .any(|existing| existing.same_registration(&wake))
        {
            waiters.retain(|existing| existing.signal.strong_count() > 0);
            waiters.push(WeakWakeHandle::new(&wake));
        }
    }

    fn register_abort(&self, abort_handle: tokio::task::AbortHandle) -> Option<AbortRegistration> {
        if self.is_cancelled() {
            abort_handle.abort();
            return None;
        }
        let mut abort_handles = self
            .abort_handles
            .lock()
            .expect("cancellation abort handle lock poisoned");
        if self.is_cancelled() {
            drop(abort_handles);
            abort_handle.abort();
            return None;
        }
        let id = self.next_abort_id.fetch_add(1, Ordering::Relaxed);
        abort_handles.insert(id, abort_handle);
        Some(AbortRegistration {
            abort_handles: Arc::downgrade(&self.abort_handles),
            id,
        })
    }

    async fn cancelled(&self) {
        let mut notified = Box::pin(self.notification.notified());
        notified.as_mut().enable();
        if !self.is_cancelled() {
            notified.await;
        }
    }
}

/// The cancel *capability*: whoever holds a `&mut RssCancellationSource` can
/// trigger cancellation. It hands out read-only [`RssCancellationToken`]
/// observation tickets that share its flag, keeping "who can cancel" explicit
/// in signatures (cancel takes `mut source`, observation takes `read token`).
#[derive(Debug, Clone, Default)]
pub struct RssCancellationSource {
    token: CancellationToken,
}

/// A read-only observation ticket. It can be copied and passed to workers and
/// loops, but only observes — it carries no cancel power.
#[derive(Debug, Clone, Default)]
pub struct RssCancellationToken {
    token: CancellationToken,
}

pub fn cancellation_source_new() -> RssCancellationSource {
    RssCancellationSource {
        token: CancellationToken::new(),
    }
}

pub fn cancellation_source_token(source: &RssCancellationSource) -> RssCancellationToken {
    RssCancellationToken {
        token: source.token.clone(),
    }
}

pub fn cancellation_source_cancel(source: &mut RssCancellationSource) {
    source.token.cancel();
}

pub fn cancellation_token_is_cancelled(token: &RssCancellationToken) -> bool {
    token.token.is_cancelled()
}

pub(crate) fn cancellation_token_register_wake(token: &RssCancellationToken, wake: WakeHandle) {
    token.token.register_wake(wake);
}

#[cfg(feature = "net")]
pub(crate) async fn cancellation_token_cancelled(token: &RssCancellationToken) {
    token.token.cancelled().await;
}

pub fn trace_async_runtime_phase(
    phase: &str,
    elapsed_us: u128,
    polls: usize,
    yields: usize,
    tasks: usize,
) {
    info!(
        phase,
        elapsed_us, polls, yields, tasks, "async_runtime_phase"
    );
}

/// A token that is never cancelled, used for `Task.cancellation_token()` outside
/// a `task_group` so cooperative checks simply never see cancellation.
pub fn cancellation_never() -> RssCancellationToken {
    RssCancellationToken::default()
}

/// The internal cancellation owner of a `task_group` scope. The group holds the
/// cancel capability; children only get read-only tokens. On any scope exit —
/// normal completion, an early `return`, or `?` error propagation — the guard
/// drops and cancels, so cooperative siblings stop instead of leaking.
pub struct TaskGroupScope {
    source: RssCancellationSource,
}

impl Default for TaskGroupScope {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskGroupScope {
    pub fn new() -> Self {
        Self {
            source: cancellation_source_new(),
        }
    }

    /// A read-only token sharing this scope's cancellation flag.
    pub fn token(&self) -> RssCancellationToken {
        cancellation_source_token(&self.source)
    }
}

impl Drop for TaskGroupScope {
    fn drop(&mut self) {
        cancellation_source_cancel(&mut self.source);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskGroupJoin<T> {
    Completed(Vec<T>),
    TimedOut {
        completed: Vec<Option<T>>,
        pending: usize,
    },
}

impl<T> TaskGroup<T> {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            cancellation: CancellationToken::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn spawn_pending(&mut self, pending: impl Pending<T> + 'static) {
        self.tasks.push(Box::new(pending));
    }

    pub fn spawn_cancellable(
        &mut self,
        create: impl FnOnce(CancellationToken) -> Box<dyn Pending<T>>,
    ) {
        self.tasks.push(create(self.cancellation.clone()));
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn join(self, executor: &mut Executor) -> Vec<T> {
        let started = Instant::now();
        let start_polls = executor.polls;
        let start_yields = executor.yields;
        let mut tasks = self.tasks;
        let task_count = tasks.len();
        let mut outputs = (0..tasks.len()).map(|_| None).collect::<Vec<_>>();
        let mut remaining = tasks.len();
        let mut poll_indices = (0..tasks.len()).collect::<Vec<_>>();
        while remaining > 0 {
            for index in std::mem::take(&mut poll_indices) {
                if outputs[index].is_some() {
                    continue;
                }
                executor.polls += 1;
                let poll = {
                    let mut cx = Context {
                        executor,
                        wake_key: Some(index),
                    };
                    tasks[index].poll(&mut cx)
                };
                if let AsyncPoll::Ready(value) = poll {
                    outputs[index] = Some(value);
                    remaining -= 1;
                }
            }
            if remaining > 0 {
                executor.park_or_yield(None);
                poll_indices = executor.drain_ready_wake_keys();
                if poll_indices.is_empty() {
                    poll_indices.extend(
                        outputs
                            .iter()
                            .enumerate()
                            .filter_map(|(index, output)| output.is_none().then_some(index)),
                    );
                } else {
                    poll_indices
                        .retain(|index| *index < outputs.len() && outputs[*index].is_none());
                }
            }
        }
        info!(
            phase = "task_group_join",
            tasks = task_count,
            elapsed_us = started.elapsed().as_micros(),
            polls = executor.polls.saturating_sub(start_polls),
            yields = executor.yields.saturating_sub(start_yields),
            "async_runtime_phase"
        );
        outputs
            .into_iter()
            .map(|output| output.expect("task group output should be ready after join"))
            .collect()
    }

    pub fn join_until(self, executor: &mut Executor, deadline: Instant) -> TaskGroupJoin<T> {
        let started = Instant::now();
        let start_polls = executor.polls;
        let start_yields = executor.yields;
        let cancellation = self.cancellation.clone();
        let mut tasks = self.tasks;
        let task_count = tasks.len();
        let mut outputs = (0..tasks.len()).map(|_| None).collect::<Vec<_>>();
        let mut remaining = tasks.len();
        let mut poll_indices = (0..tasks.len()).collect::<Vec<_>>();
        while remaining > 0 {
            if Instant::now() >= deadline {
                cancellation.cancel();
                info!(
                    phase = "task_group_join_timeout",
                    tasks = task_count,
                    elapsed_us = started.elapsed().as_micros(),
                    polls = executor.polls.saturating_sub(start_polls),
                    yields = executor.yields.saturating_sub(start_yields),
                    pending = remaining,
                    "async_runtime_phase"
                );
                return TaskGroupJoin::TimedOut {
                    completed: outputs,
                    pending: remaining,
                };
            }
            for index in std::mem::take(&mut poll_indices) {
                if outputs[index].is_some() {
                    continue;
                }
                executor.polls += 1;
                let poll = {
                    let mut cx = Context {
                        executor,
                        wake_key: Some(index),
                    };
                    tasks[index].poll(&mut cx)
                };
                if let AsyncPoll::Ready(value) = poll {
                    outputs[index] = Some(value);
                    remaining -= 1;
                }
            }
            if remaining > 0 {
                executor.park_or_yield(Some(deadline));
                poll_indices = executor.drain_ready_wake_keys();
                if poll_indices.is_empty() {
                    poll_indices.extend(
                        outputs
                            .iter()
                            .enumerate()
                            .filter_map(|(index, output)| output.is_none().then_some(index)),
                    );
                } else {
                    poll_indices
                        .retain(|index| *index < outputs.len() && outputs[*index].is_none());
                }
            }
        }
        info!(
            phase = "task_group_join_until",
            tasks = task_count,
            elapsed_us = started.elapsed().as_micros(),
            polls = executor.polls.saturating_sub(start_polls),
            yields = executor.yields.saturating_sub(start_yields),
            "async_runtime_phase"
        );
        TaskGroupJoin::Completed(
            outputs
                .into_iter()
                .map(|output| output.expect("task group output should be ready after join"))
                .collect(),
        )
    }
}

impl<T> Default for TaskGroup<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TimerSleepPending {
    deadline: Instant,
}

impl Pending<Result<(), TimerError>> for TimerSleepPending {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<Result<(), TimerError>> {
        let now = Instant::now();
        if now >= self.deadline {
            return AsyncPoll::Ready(Ok(()));
        }
        let remaining = self.deadline.saturating_duration_since(now);
        cx.wake_after(remaining);
        AsyncPoll::Pending
    }
}

pub fn timer_sleep_start(ms: i64) -> TimerSleepPending {
    let millis = u64::try_from(ms).unwrap_or(0);
    TimerSleepPending {
        deadline: Instant::now() + Duration::from_millis(millis),
    }
}

pub fn timer_sleep_native_start(ms: i64) -> NativeAsyncPending<Result<(), TimerError>> {
    timer_sleep_native_start_with_cancellation(ms, CancellationToken::new())
}

/// Sleep until a monotonic [`crate::clock::RssDeadline`] passes. Bridges the
/// cooperative `Deadline` primitive to the existing async sleep: an already
/// expired deadline yields immediately (zero remaining).
pub fn timer_sleep_until_native_start(
    deadline: &crate::clock::RssDeadline,
) -> NativeAsyncPending<Result<(), TimerError>> {
    timer_sleep_native_start(crate::clock::deadline_remaining_ms(deadline))
}

/// Sleep up to `ms`, returning `Ok(())` early as soon as `token` is cancelled.
/// Unlike the task-group cancellation (which abandons a pending), this always
/// completes — so an awaited cancellable sleep wakes promptly instead of
/// hanging. The caller checks the token to learn whether it elapsed or woke on
/// cancel. This is the building block for a background loop that sleeps between
/// iterations but stops immediately on shutdown.
pub fn timer_sleep_cancellable_native_start(
    ms: i64,
    token: &RssCancellationToken,
) -> NativeAsyncPending<Result<(), TimerError>> {
    let cancellation = token.token.clone();
    let duration = Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    spawn_tokio_native(async move {
        let sleep = Box::pin(tokio::time::sleep(duration));
        let cancelled = Box::pin(cancellation.cancelled());
        let _completed_first = futures_util::future::select(sleep, cancelled).await;
        Ok(())
    })
}

pub fn timer_sleep_native_start_with_cancellation(
    ms: i64,
    cancellation: CancellationToken,
) -> NativeAsyncPending<Result<(), TimerError>> {
    let millis = u64::try_from(ms).unwrap_or(0);
    spawn_tokio_native_with_cancellation(cancellation, async move {
        tokio::time::sleep(Duration::from_millis(millis)).await;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn tokio_native_pending_completes_on_runtime() {
        let mut executor = Executor::new();
        let pending = spawn_tokio_native(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            42
        });

        assert_eq!(executor.run_pending(pending), 42);
    }

    #[test]
    fn dropping_tokio_native_pending_aborts_backend_work() {
        struct DropSignal(mpsc::Sender<()>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let (started_sender, started_receiver) = mpsc::channel();
        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let pending = spawn_tokio_native(async move {
            let _signal = DropSignal(dropped_sender);
            started_sender.send(()).expect("test receiver should exist");
            std::future::pending::<()>().await;
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Tokio task should start");

        drop(pending);

        dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("dropping pending should abort and drop the Tokio future");
    }

    #[test]
    fn cancelling_tokio_native_pending_aborts_backend_work() {
        struct DropSignal(mpsc::Sender<()>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let cancellation = CancellationToken::new();
        let (started_sender, started_receiver) = mpsc::channel();
        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let _pending = spawn_tokio_native_with_cancellation(cancellation.clone(), async move {
            let _signal = DropSignal(dropped_sender);
            started_sender.send(()).expect("test receiver should exist");
            std::future::pending::<()>().await;
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Tokio task should start");

        cancellation.cancel();

        dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("cancellation should abort and drop the Tokio future");
    }

    #[test]
    fn completed_native_tasks_deregister_cancellation_abort_handles() {
        let cancellation = CancellationToken::new();
        let mut group = TaskGroup::new();
        for value in 0..256 {
            group.spawn_pending(spawn_tokio_native_with_cancellation(
                cancellation.clone(),
                async move { value },
            ));
        }

        let mut executor = Executor::new();
        assert_eq!(group.join(&mut executor).len(), 256);
        assert!(
            cancellation
                .abort_handles
                .lock()
                .expect("cancellation abort handle lock poisoned")
                .is_empty(),
            "completed tasks must not remain registered for cancellation"
        );
    }

    #[test]
    fn dropping_native_tasks_deregisters_cancellation_abort_handles() {
        let cancellation = CancellationToken::new();
        let pending = spawn_tokio_native_with_cancellation(cancellation.clone(), async {
            std::future::pending::<()>().await
        });
        assert_eq!(
            cancellation
                .abort_handles
                .lock()
                .expect("cancellation abort handle lock poisoned")
                .len(),
            1
        );

        drop(pending);

        assert!(
            cancellation
                .abort_handles
                .lock()
                .expect("cancellation abort handle lock poisoned")
                .is_empty()
        );
    }

    #[test]
    fn ready_wake_keys_are_deduplicated() {
        let mut executor = Executor::new();
        let wake = executor.wake_handle().with_key(Some(17));
        for _ in 0..10_000 {
            wake.wake();
        }

        assert_eq!(executor.drain_ready_wake_keys(), vec![17]);
        assert!(executor.drain_ready_wake_keys().is_empty());
    }

    #[test]
    fn runtime_worker_count_is_configurable_and_bounded() {
        assert_eq!(bounded_worker_threads(None, 0), 1);
        assert_eq!(bounded_worker_threads(None, usize::MAX), 32);
        assert_eq!(bounded_worker_threads(Some("8"), 1), 8);
        assert_eq!(bounded_worker_threads(Some("0"), 8), 1);
        assert_eq!(bounded_worker_threads(Some("1000"), 8), 32);
        assert_eq!(bounded_worker_threads(Some("invalid"), 6), 6);
    }

    #[test]
    fn cancellation_token_observes_source_cancel() {
        let mut source = cancellation_source_new();
        let token = cancellation_source_token(&source);
        assert!(!cancellation_token_is_cancelled(&token));
        cancellation_source_cancel(&mut source);
        assert!(cancellation_token_is_cancelled(&token));
    }

    #[test]
    fn all_tokens_from_one_source_observe_cancel() {
        let mut source = cancellation_source_new();
        let first = cancellation_source_token(&source);
        let second = cancellation_source_token(&source);
        cancellation_source_cancel(&mut source);
        assert!(cancellation_token_is_cancelled(&first));
        assert!(cancellation_token_is_cancelled(&second));
    }

    #[test]
    fn pending_try_chains_and_short_circuits() {
        let mut executor = Executor::new();
        let ok: Result<i64, i64> = Ok(2);
        let chain = pending_try(pending_ready(ok), |x: i64| {
            pending_ready::<Result<i64, i64>>(Ok(x + 1))
        });
        assert_eq!(executor.run_pending(chain), Ok(3));

        let err: Result<i64, i64> = Err(7);
        let chain = pending_try(pending_ready(err), |x: i64| {
            pending_ready::<Result<i64, i64>>(Ok(x + 1))
        });
        assert_eq!(executor.run_pending(chain), Err(7));
    }

    #[test]
    fn pending_defer_runs_boundary_once() {
        let runs = Arc::new(AtomicUsize::new(0));
        let runs_for_pending = runs.clone();
        let mut executor = Executor::new();
        let value = executor.run_pending(pending_defer(move || {
            runs_for_pending.fetch_add(1, Ordering::SeqCst);
            42
        }));

        assert_eq!(value, 42);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn task_group_poll_loop_runs_siblings_concurrently() {
        // Mirrors the generated task_group poll loop: two 50ms sleeps driven
        // together should finish in ~50ms, not ~100ms (proving interleaving).
        let mut executor = Executor::new();
        let mut a = timer_sleep_native_start(50);
        let mut b = timer_sleep_native_start(50);
        let mut result_a = None;
        let mut result_b = None;
        let start = std::time::Instant::now();
        loop {
            let mut progress = false;
            if result_a.is_none()
                && let AsyncPoll::Ready(value) = executor.poll_once(&mut a)
            {
                result_a = Some(value);
                progress = true;
            }
            if result_b.is_none()
                && let AsyncPoll::Ready(value) = executor.poll_once(&mut b)
            {
                result_b = Some(value);
                progress = true;
            }
            if result_a.is_some() && result_b.is_some() {
                break;
            }
            if !progress {
                executor.yield_once();
            }
        }
        let elapsed = start.elapsed();
        assert!(result_a.unwrap().is_ok() && result_b.unwrap().is_ok());
        assert!(
            elapsed.as_millis() < 90,
            "two concurrent 50ms sleeps took {}ms (sequential would be ~100ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn task_group_scope_cancels_token_on_drop() {
        let token = {
            let scope = TaskGroupScope::new();
            let token = scope.token();
            assert!(!cancellation_token_is_cancelled(&token));
            token
        };
        // Dropping the scope (normal or early exit) cancels every group token.
        assert!(cancellation_token_is_cancelled(&token));
    }

    #[test]
    fn never_cancelled_token_is_never_cancelled() {
        assert!(!cancellation_token_is_cancelled(&cancellation_never()));
    }

    #[test]
    fn distinct_sources_do_not_share_cancel() {
        let mut source = cancellation_source_new();
        let other = cancellation_source_new();
        let other_token = cancellation_source_token(&other);
        cancellation_source_cancel(&mut source);
        assert!(!cancellation_token_is_cancelled(&other_token));
    }

    #[test]
    fn timer_sleep_cancellable_completes_when_not_cancelled() {
        let mut executor = Executor::new();
        let token = cancellation_source_token(&cancellation_source_new());
        let pending = timer_sleep_cancellable_native_start(5, &token);
        assert!(executor.run_pending(pending).is_ok());
    }

    #[test]
    fn timer_sleep_cancellable_wakes_early_on_cancel() {
        let mut executor = Executor::new();
        let mut source = cancellation_source_new();
        let token = cancellation_source_token(&source);
        cancellation_source_cancel(&mut source);
        // A 100s sleep that is already cancelled must complete promptly; if it
        // did not, this test would hang rather than fail.
        let pending = timer_sleep_cancellable_native_start(100_000, &token);
        assert!(executor.run_pending(pending).is_ok());
    }

    #[test]
    fn native_timers_share_the_tokio_runtime() {
        let mut executor = Executor::new();
        let timers = (0..1_000)
            .map(|_| timer_sleep_native_start(1))
            .collect::<Vec<_>>();
        for timer in timers {
            assert!(executor.run_pending(timer).is_ok());
        }
    }

    #[test]
    fn high_count_native_timers_use_keyed_task_group_wakes() {
        const TASKS: usize = 1_000;

        let mut group = TaskGroup::new();
        for value in 0..TASKS {
            group.spawn_pending(pending_then(timer_sleep_native_start(5), move |result| {
                assert!(result.is_ok());
                pending_ready(value)
            }));
        }

        let mut executor = Executor::new();
        let outputs = group.join(&mut executor);

        assert_eq!(outputs.len(), TASKS);
        assert_eq!(outputs[0], 0);
        assert_eq!(outputs[TASKS - 1], TASKS - 1);
        assert!(
            executor.poll_count() <= TASKS * 4,
            "{} timers required {} polls",
            TASKS,
            executor.poll_count()
        );
    }

    #[test]
    fn timer_sleep_until_deadline_completes() {
        let mut executor = Executor::new();
        let deadline = crate::clock::deadline_after_ms(5);
        let pending = timer_sleep_until_native_start(&deadline);
        assert!(executor.run_pending(pending).is_ok());
    }

    #[test]
    fn timer_sleep_until_expired_deadline_completes_immediately() {
        let mut executor = Executor::new();
        let deadline = crate::clock::deadline_after_ms(0);
        let pending = timer_sleep_until_native_start(&deadline);
        assert!(executor.run_pending(pending).is_ok());
    }

    #[test]
    fn tokio_native_pending_can_run_concurrently_in_task_group() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let mut group = TaskGroup::new();

        for index in 0..8 {
            let in_flight = in_flight.clone();
            let max_in_flight = max_in_flight.clone();
            group.spawn_pending(spawn_tokio_native(async move {
                let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(40)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                index
            }));
        }

        let mut executor = Executor::new();
        let mut outputs = group.join(&mut executor);
        outputs.sort();

        assert_eq!(outputs, (0..8).collect::<Vec<_>>());
        assert!(
            max_in_flight.load(Ordering::SeqCst) > 1,
            "tokio-backed native pending work should overlap"
        );
    }

    #[test]
    fn runtime_services_execute_and_shutdown_independently_in_parallel() {
        let first = Arc::new(RuntimeServices::new().expect("first services"));
        let second = Arc::new(RuntimeServices::new().expect("second services"));
        let threads = [first.clone(), second.clone()].map(|services| {
            std::thread::spawn(move || {
                let pending =
                    spawn_tokio_native_with_services(&services, CancellationToken::new(), async {
                        42
                    })
                    .expect("service spawn");
                Executor::new().run_pending(pending)
            })
        });
        assert_eq!(
            threads.map(|thread| thread.join().expect("service thread")),
            [42, 42]
        );

        first.shutdown(Duration::from_secs(1));
        assert!(first.is_shutdown());
        assert!(!second.is_shutdown());
        assert!(
            spawn_tokio_native_with_services(&first, CancellationToken::new(), async { 1 })
                .is_err()
        );
        let pending =
            spawn_tokio_native_with_services(&second, CancellationToken::new(), async { 7 })
                .expect("second remains live");
        assert_eq!(Executor::new().run_pending(pending), 7);
        second.shutdown(Duration::from_secs(1));
    }

    #[test]
    fn runtime_services_keep_configuration_per_instance() {
        let first = RuntimeServices::with_worker_threads(2).expect("first services");
        let second = RuntimeServices::with_worker_threads(3).expect("second services");

        assert_eq!(first.worker_threads(), 2);
        assert_eq!(second.worker_threads(), 3);
        first.shutdown(Duration::from_secs(1));
        second.shutdown(Duration::from_secs(1));
    }

    #[test]
    fn pending_owns_runtime_services_until_completion() {
        let services = Arc::new(RuntimeServices::new().expect("runtime services"));
        let weak = Arc::downgrade(&services);
        let pending =
            spawn_tokio_native_with_services(&services, CancellationToken::new(), async { 42 })
                .expect("service spawn");

        drop(services);
        assert!(weak.upgrade().is_some(), "pending must retain its runtime");
        assert_eq!(Executor::new().run_pending(pending), 42);
        assert!(
            weak.upgrade().is_none(),
            "completed pending releases runtime"
        );
    }
}
