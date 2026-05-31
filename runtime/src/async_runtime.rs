use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::domain::TimerError;

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
        loop {
            self.polls += 1;
            let poll = {
                let mut cx = Context { executor: self };
                pending.poll(&mut cx)
            };
            match poll {
                AsyncPoll::Ready(value) => return value,
                AsyncPoll::Pending => {
                    self.park_or_yield(None);
                }
            }
        }
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
        if let Some(deadline) = target {
            let now = Instant::now();
            if deadline > now {
                self.wake_signal
                    .wait_timeout(deadline.saturating_duration_since(now));
                return;
            }
        }
        std::thread::yield_now();
    }
}

#[derive(Debug, Clone, Default)]
pub struct WakeHandle {
    signal: Arc<WakeSignal>,
}

#[derive(Debug, Default)]
struct WakeSignal {
    woken: Mutex<bool>,
    ready: Condvar,
}

impl WakeHandle {
    pub fn wake(&self) {
        let mut woken = self.signal.woken.lock().expect("wake signal lock poisoned");
        *woken = true;
        self.signal.ready.notify_all();
    }

    fn wait_timeout(&self, duration: Duration) {
        let mut woken = self.signal.woken.lock().expect("wake signal lock poisoned");
        if *woken {
            *woken = false;
            return;
        }
        let (mut woken_after_wait, _) = self
            .signal
            .ready
            .wait_timeout(woken, duration)
            .expect("wake signal condvar wait poisoned");
        if *woken_after_wait {
            *woken_after_wait = false;
        }
    }
}

pub struct Context<'executor> {
    executor: &'executor mut Executor,
}

impl Context<'_> {
    pub fn yield_now(&mut self) {
        self.executor.yields += 1;
        std::thread::yield_now();
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
        self.executor.wake_handle()
    }
}

pub trait Pending<T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<T>;
}

pub fn run_pending<T>(pending: impl Pending<T>) -> T {
    Executor::new().run_pending(pending)
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
        let mut tasks = self.tasks;
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
                    let mut cx = Context { executor };
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
        outputs
            .into_iter()
            .map(|output| output.expect("task group output should be ready after join"))
            .collect()
    }

    pub fn join_until(self, executor: &mut Executor, deadline: Instant) -> TaskGroupJoin<T> {
        let cancellation = self.cancellation.clone();
        let mut tasks = self.tasks;
        let mut outputs = (0..tasks.len()).map(|_| None).collect::<Vec<_>>();
        let mut remaining = tasks.len();
        while remaining > 0 {
            if Instant::now() >= deadline {
                cancellation.cancel();
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
                    let mut cx = Context { executor };
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
