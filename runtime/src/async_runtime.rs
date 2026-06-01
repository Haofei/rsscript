use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
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
}

impl WakeHandle {
    pub fn wake(&self) {
        let mut state = self.signal.state.lock().expect("wake signal lock poisoned");
        state.woken = true;
        if let Some(key) = self.key {
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
        std::mem::take(&mut state.ready_keys)
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
        std::thread::sleep(duration);
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
    cancellation: CancellationToken,
    wake: Arc<Mutex<Option<WakeHandle>>>,
}

#[derive(Clone)]
pub struct NativeAsyncCompleter<T> {
    result: Arc<Mutex<Option<T>>>,
    cancellation: CancellationToken,
    wake: Arc<Mutex<Option<WakeHandle>>>,
}

pub fn native_async_pending<T>(
    cancellation: CancellationToken,
) -> (NativeAsyncPending<T>, NativeAsyncCompleter<T>) {
    let result = Arc::new(Mutex::new(None));
    let wake = Arc::new(Mutex::new(None));
    (
        NativeAsyncPending {
            result: result.clone(),
            cancellation: cancellation.clone(),
            wake: wake.clone(),
        },
        NativeAsyncCompleter {
            result,
            cancellation,
            wake,
        },
    )
}

static TOKIO_NATIVE_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> =
    std::sync::OnceLock::new();

pub fn tokio_native_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_NATIVE_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("rsscript-runtime-tokio")
            .enable_all()
            .build()
            .expect("rsscript tokio runtime should start")
    })
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
    let (pending, completer) = native_async_pending(cancellation);
    tokio_native_runtime().spawn(async move {
        let value = future.await;
        completer.complete(value);
    });
    pending
}

impl<T> NativeAsyncCompleter<T> {
    pub fn complete(&self, value: T) -> bool {
        if self.cancellation.is_cancelled() {
            return false;
        }
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
        true
    }
}

impl<T> Pending<T> for NativeAsyncPending<T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<T> {
        if self.cancellation.is_cancelled() {
            return AsyncPoll::Pending;
        }
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

pub struct TaskGroup<T> {
    tasks: Vec<Box<dyn Pending<T>>>,
    cancellation: CancellationToken,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
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
        while remaining > 0 {
            let mut made_progress = false;
            for (index, task) in tasks.iter_mut().enumerate() {
                if outputs[index].is_some() {
                    continue;
                }
                executor.polls += 1;
                let poll = {
                    let mut cx = Context {
                        executor,
                        wake_key: Some(index),
                    };
                    task.poll(&mut cx)
                };
                if let AsyncPoll::Ready(value) = poll {
                    outputs[index] = Some(value);
                    remaining -= 1;
                    made_progress = true;
                }
            }
            if remaining > 0 && !made_progress {
                executor.park_or_yield(None);
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
            let mut made_progress = false;
            for (index, task) in tasks.iter_mut().enumerate() {
                if outputs[index].is_some() {
                    continue;
                }
                executor.polls += 1;
                let poll = {
                    let mut cx = Context {
                        executor,
                        wake_key: Some(index),
                    };
                    task.poll(&mut cx)
                };
                if let AsyncPoll::Ready(value) = poll {
                    outputs[index] = Some(value);
                    remaining -= 1;
                    made_progress = true;
                }
            }
            if remaining > 0 && !made_progress {
                executor.park_or_yield(Some(deadline));
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
    let deadline = Instant::now() + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    let cancellation = token.token.clone();
    // A fresh internal token keeps completion ungated; we observe the external
    // cancellation ourselves and complete early when it fires.
    let (pending, completer) = native_async_pending(CancellationToken::new());
    std::thread::spawn(move || {
        let tick = Duration::from_millis(5);
        loop {
            let now = Instant::now();
            if cancellation.is_cancelled() || now >= deadline {
                let _completed = completer.complete(Ok(()));
                return;
            }
            std::thread::sleep(tick.min(deadline - now));
        }
    });
    pending
}

pub fn timer_sleep_native_start_with_cancellation(
    ms: i64,
    cancellation: CancellationToken,
) -> NativeAsyncPending<Result<(), TimerError>> {
    let millis = u64::try_from(ms).unwrap_or(0);
    let (pending, completer) = native_async_pending(cancellation.clone());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(millis));
        if !cancellation.is_cancelled() {
            let _completed = completer.complete(Ok(()));
        }
    });
    pending
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
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
            if result_a.is_none() {
                if let AsyncPoll::Ready(value) = executor.poll_once(&mut a) {
                    result_a = Some(value);
                    progress = true;
                }
            }
            if result_b.is_none() {
                if let AsyncPoll::Ready(value) = executor.poll_once(&mut b) {
                    result_b = Some(value);
                    progress = true;
                }
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
}
