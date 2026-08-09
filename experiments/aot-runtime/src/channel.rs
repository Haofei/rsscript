//! Bounded MPSC channel — a cooperative, single-isolate producer/consumer
//! primitive. `send`/`recv` are pendings that yield (return `Pending`) when the
//! channel is full/empty; the enclosing `task_group` poll loop then drives the
//! sibling task, so a full channel never deadlocks. No `Send` bound and no Tokio:
//! the shared state is `Rc<RefCell<..>>`, matching RSScript's single isolate.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, mpsc as std_mpsc};
use std::time::Duration;

use crate::async_runtime::{
    AsyncPoll, Context, Pending, RssCancellationToken, WakeHandle, cancellation_token_is_cancelled,
    cancellation_token_register_wake,
};
use crate::{RssDeadline, cancellation_never, deadline_after_ms, deadline_remaining_duration};

const DEFAULT_STREAM_COLLECT_MAX_ITEMS: usize = 1_000_000;
const DEFAULT_STREAM_COLLECT_TIMEOUT_MS: i64 = 60_000;
const STREAM_COLLECT_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Opaque channel error surfaced to RSScript. (Variant matching — `Closed` vs
/// `Cancelled` vs `InvalidCapacity` — is deferred; for now the message
/// distinguishes them and `?` propagation is the main pattern.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelError {
    message: String,
}

impl ChannelError {
    pub(crate) fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

pub fn channel_error_message(error: &ChannelError) -> String {
    error.message.clone()
}

struct ChannelState<T> {
    capacity: usize,
    queue: VecDeque<T>,
    senders: usize,
    receiver_taken: bool,
    receiver_closed: bool,
}

pub struct RssChannel<T> {
    state: Rc<RefCell<ChannelState<T>>>,
}

pub struct RssSender<T> {
    state: Rc<RefCell<ChannelState<T>>>,
    // Per-sender closed flag, shared with the `SendPending`s it hands out so a
    // `send` started before `close` still observes the closure. Each sender
    // instance owns one slot in the channel's `senders` count; `close` (or
    // `Drop`) releases it exactly once.
    closed: Rc<Cell<bool>>,
}

pub struct RssReceiver<T> {
    state: Rc<RefCell<ChannelState<T>>>,
}

pub struct RssStream<T> {
    backend: RefCell<RssStreamBackend<T>>,
    on_drop: Option<Box<dyn FnOnce()>>,
}

enum RssStreamBackend<T> {
    Receiver(RssReceiver<T>),
    Iterator(Box<dyn Iterator<Item = Result<T, ChannelError>>>),
    External(Arc<ExternalStream<T>>),
}

struct ExternalStream<T> {
    state: Mutex<ExternalStreamState<T>>,
    ready: Condvar,
}

struct ExternalStreamState<T> {
    items: VecDeque<Result<T, ChannelError>>,
    disconnected: bool,
    stopped: bool,
    wake: Option<WakeHandle>,
}

const EXTERNAL_STREAM_CAPACITY: usize = 64;
const EXTERNAL_STREAM_POLL_INTERVAL: Duration = Duration::from_millis(25);

// A `Sender` clone is another producer with its own closed flag and its own slot
// in the `senders` count; the receiver drains and returns `None` only once every
// sender is gone.
impl<T> Clone for RssSender<T> {
    fn clone(&self) -> Self {
        self.state.borrow_mut().senders += 1;
        Self {
            state: Rc::clone(&self.state),
            closed: Rc::new(Cell::new(false)),
        }
    }
}

// Dropping a still-open sender releases its slot, just like `close`. This keeps
// the `senders` count honest when a producer goes out of scope without an
// explicit `close` (e.g. a `task_group` task that finishes).
impl<T> Drop for RssSender<T> {
    fn drop(&mut self) {
        if !self.closed.replace(true) {
            let mut state = self.state.borrow_mut();
            state.senders = state.senders.saturating_sub(1);
        }
    }
}

pub fn channel_bounded<T>(capacity: i64) -> Result<RssChannel<T>, ChannelError> {
    const MAX_CHANNEL_CAPACITY: usize = 1_000_000;
    if capacity <= 0 {
        return Err(ChannelError::new("channel capacity must be positive"));
    }
    let capacity = usize::try_from(capacity)
        .map_err(|_| ChannelError::new("channel capacity does not fit this platform"))?;
    if capacity > MAX_CHANNEL_CAPACITY {
        return Err(ChannelError::new(&format!(
            "channel capacity {capacity} exceeds runtime ceiling of {MAX_CHANNEL_CAPACITY} items"
        )));
    }
    Ok(RssChannel {
        state: Rc::new(RefCell::new(ChannelState {
            capacity,
            queue: VecDeque::new(),
            senders: 0,
            receiver_taken: false,
            receiver_closed: false,
        })),
    })
}

pub fn channel_sender<T>(channel: &RssChannel<T>) -> RssSender<T> {
    channel.state.borrow_mut().senders += 1;
    RssSender {
        state: Rc::clone(&channel.state),
        closed: Rc::new(Cell::new(false)),
    }
}

pub fn channel_receiver<T>(channel: &mut RssChannel<T>) -> Result<RssReceiver<T>, ChannelError> {
    {
        let mut state = channel.state.borrow_mut();
        if state.receiver_taken {
            return Err(ChannelError::new("channel receiver already taken"));
        }
        state.receiver_taken = true;
    }
    Ok(RssReceiver {
        state: Rc::clone(&channel.state),
    })
}

pub fn sender_close<T>(sender: &mut RssSender<T>) {
    // Idempotent and per-sender: only the first close (whether explicit here or
    // via `Drop`) releases this sender's slot. After close, the sender's pending
    // sends observe `closed` and fail rather than silently enqueueing.
    if !sender.closed.replace(true) {
        let mut state = sender.state.borrow_mut();
        state.senders = state.senders.saturating_sub(1);
    }
}

pub fn receiver_close<T>(receiver: &mut RssReceiver<T>) {
    receiver.state.borrow_mut().receiver_closed = true;
}

pub fn receiver_into_stream<T>(receiver: RssReceiver<T>) -> RssStream<T> {
    RssStream {
        backend: RefCell::new(RssStreamBackend::Receiver(receiver)),
        on_drop: None,
    }
}

pub fn stream_from_list<T: 'static>(items: Vec<T>) -> RssStream<T> {
    RssStream {
        backend: RefCell::new(RssStreamBackend::Iterator(Box::new(
            items.into_iter().map(Ok),
        ))),
        on_drop: None,
    }
}

pub fn stream_from_iterator<T: 'static>(
    items: impl Iterator<Item = Result<T, ChannelError>> + 'static,
) -> RssStream<T> {
    RssStream {
        backend: RefCell::new(RssStreamBackend::Iterator(Box::new(items))),
        on_drop: None,
    }
}

pub fn stream_from_external_receiver<T: Send + 'static>(
    receiver: std_mpsc::Receiver<Result<T, ChannelError>>,
) -> RssStream<T> {
    stream_from_external_receiver_with_drop(receiver, None)
}

pub(crate) fn stream_from_external_receiver_with_drop<T: Send + 'static>(
    receiver: std_mpsc::Receiver<Result<T, ChannelError>>,
    on_drop: Option<Box<dyn FnOnce()>>,
) -> RssStream<T> {
    let external = Arc::new(ExternalStream {
        state: Mutex::new(ExternalStreamState {
            items: VecDeque::new(),
            disconnected: false,
            stopped: false,
            wake: None,
        }),
        ready: Condvar::new(),
    });
    let producer = Arc::clone(&external);
    std::thread::spawn(move || {
        loop {
            let item = match receiver.recv_timeout(EXTERNAL_STREAM_POLL_INTERVAL) {
                Ok(item) => item,
                Err(std_mpsc::RecvTimeoutError::Timeout) => {
                    if producer
                        .state
                        .lock()
                        .expect("external stream lock poisoned")
                        .stopped
                    {
                        return;
                    }
                    continue;
                }
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let wake = {
                let mut state = producer
                    .state
                    .lock()
                    .expect("external stream lock poisoned");
                while state.items.len() >= EXTERNAL_STREAM_CAPACITY && !state.stopped {
                    state = producer
                        .ready
                        .wait(state)
                        .expect("external stream condvar wait poisoned");
                }
                if state.stopped {
                    return;
                }
                state.items.push_back(item);
                producer.ready.notify_all();
                state.wake.take()
            };
            if let Some(wake) = wake {
                wake.wake();
            }
        }
        let wake = {
            let mut state = producer
                .state
                .lock()
                .expect("external stream lock poisoned");
            state.disconnected = true;
            producer.ready.notify_all();
            state.wake.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    });
    RssStream {
        backend: RefCell::new(RssStreamBackend::External(external)),
        on_drop,
    }
}

impl<T> Drop for RssStream<T> {
    fn drop(&mut self) {
        if let RssStreamBackend::External(external) = self.backend.get_mut() {
            let wake = {
                let mut state = external
                    .state
                    .lock()
                    .expect("external stream lock poisoned");
                state.stopped = true;
                state.items.clear();
                external.ready.notify_all();
                state.wake.take()
            };
            if let Some(wake) = wake {
                wake.wake();
            }
        }
        if let Some(on_drop) = self.on_drop.take() {
            on_drop();
        }
    }
}

pub struct SendPending<T> {
    state: Rc<RefCell<ChannelState<T>>>,
    closed: Rc<Cell<bool>>,
    value: Option<T>,
    cancellation: Option<RssCancellationToken>,
}

pub fn sender_send<T>(sender: &RssSender<T>, value: T) -> SendPending<T> {
    SendPending {
        state: Rc::clone(&sender.state),
        closed: Rc::clone(&sender.closed),
        value: Some(value),
        cancellation: None,
    }
}

pub fn sender_send_cancellable<T>(
    sender: &RssSender<T>,
    value: T,
    token: &RssCancellationToken,
) -> SendPending<T> {
    SendPending {
        state: Rc::clone(&sender.state),
        closed: Rc::clone(&sender.closed),
        value: Some(value),
        cancellation: Some(token.clone()),
    }
}

impl<T> Pending<Result<(), ChannelError>> for SendPending<T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<Result<(), ChannelError>> {
        if let Some(token) = &self.cancellation
            && crate::async_runtime::cancellation_token_is_cancelled(token)
        {
            return AsyncPoll::Ready(Err(ChannelError::new("channel send cancelled")));
        }
        if self.closed.get() {
            return AsyncPoll::Ready(Err(ChannelError::new("channel sender closed")));
        }
        let mut state = self.state.borrow_mut();
        if state.receiver_closed {
            return AsyncPoll::Ready(Err(ChannelError::new("channel closed")));
        }
        if state.queue.len() < state.capacity {
            state.queue.push_back(
                self.value
                    .take()
                    .expect("send pending polled after completion"),
            );
            return AsyncPoll::Ready(Ok(()));
        }
        if let Some(token) = &self.cancellation {
            cancellation_token_register_wake(token, cx.wake_handle());
        }
        AsyncPoll::Pending
    }
}

pub struct RecvPending<T> {
    state: Rc<RefCell<ChannelState<T>>>,
    cancellation: Option<RssCancellationToken>,
}

pub fn receiver_recv<T>(receiver: &RssReceiver<T>) -> RecvPending<T> {
    RecvPending {
        state: Rc::clone(&receiver.state),
        cancellation: None,
    }
}

pub fn stream_next<T>(stream: &RssStream<T>) -> StreamNextPending<'_, T> {
    StreamNextPending { stream }
}

pub fn receiver_recv_cancellable<T>(
    receiver: &RssReceiver<T>,
    token: &RssCancellationToken,
) -> RecvPending<T> {
    RecvPending {
        state: Rc::clone(&receiver.state),
        cancellation: Some(token.clone()),
    }
}

impl<T> Pending<Result<Option<T>, ChannelError>> for RecvPending<T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<Result<Option<T>, ChannelError>> {
        if let Some(token) = &self.cancellation
            && crate::async_runtime::cancellation_token_is_cancelled(token)
        {
            return AsyncPoll::Ready(Err(ChannelError::new("channel recv cancelled")));
        }
        let mut state = self.state.borrow_mut();
        if let Some(value) = state.queue.pop_front() {
            return AsyncPoll::Ready(Ok(Some(value)));
        }
        if state.senders == 0 {
            // All senders gone and the queue is drained: end of stream.
            return AsyncPoll::Ready(Ok(None));
        }
        if let Some(token) = &self.cancellation {
            cancellation_token_register_wake(token, cx.wake_handle());
        }
        AsyncPoll::Pending
    }
}

pub struct StreamNextPending<'a, T> {
    stream: &'a RssStream<T>,
}

impl<T> Pending<Result<Option<T>, ChannelError>> for StreamNextPending<'_, T> {
    fn poll(&mut self, cx: &mut Context<'_>) -> AsyncPoll<Result<Option<T>, ChannelError>> {
        let mut backend = self.stream.backend.borrow_mut();
        match &mut *backend {
            RssStreamBackend::Receiver(receiver) => receiver_recv(receiver).poll(cx),
            RssStreamBackend::Iterator(iterator) => AsyncPoll::Ready(iterator.next().transpose()),
            RssStreamBackend::External(external) => {
                let mut state = external
                    .state
                    .lock()
                    .expect("external stream lock poisoned");
                if let Some(item) = state.items.pop_front() {
                    external.ready.notify_all();
                    AsyncPoll::Ready(item.map(Some))
                } else if state.disconnected {
                    AsyncPoll::Ready(Ok(None))
                } else {
                    state.wake = Some(cx.wake_handle());
                    AsyncPoll::Pending
                }
            }
        }
    }
}

pub fn stream_collect_list<T>(stream: &RssStream<T>) -> Result<Vec<T>, ChannelError> {
    stream_collect_list_with_limits(
        stream,
        DEFAULT_STREAM_COLLECT_MAX_ITEMS,
        &cancellation_never(),
        &deadline_after_ms(DEFAULT_STREAM_COLLECT_TIMEOUT_MS),
    )
}

pub fn stream_collect_list_with_limits<T>(
    stream: &RssStream<T>,
    max_items: usize,
    cancellation: &RssCancellationToken,
    deadline: &RssDeadline,
) -> Result<Vec<T>, ChannelError> {
    if max_items == 0 {
        return Err(ChannelError::new(
            "stream collect_list max_items must be positive",
        ));
    }
    let mut values = Vec::new();
    let mut backend = stream.backend.borrow_mut();
    match &mut *backend {
        RssStreamBackend::Iterator(iterator) => {
            for item in iterator {
                collect_push(&mut values, item?, max_items)?;
            }
            Ok(values)
        }
        RssStreamBackend::Receiver(receiver) => {
            let mut state = receiver.state.borrow_mut();
            while let Some(value) = state.queue.pop_front() {
                collect_push(&mut values, value, max_items)?;
            }
            if state.senders == 0 {
                Ok(values)
            } else {
                Err(ChannelError::new(
                    "stream collect_list would block on an open channel stream",
                ))
            }
        }
        RssStreamBackend::External(external) => {
            let mut state = external
                .state
                .lock()
                .expect("external stream lock poisoned");
            loop {
                if cancellation_token_is_cancelled(cancellation) {
                    return Err(ChannelError::new("stream collect_list was cancelled"));
                }
                while let Some(item) = state.items.pop_front() {
                    external.ready.notify_all();
                    collect_push(&mut values, item?, max_items)?;
                }
                if state.disconnected {
                    return Ok(values);
                }
                let remaining = deadline_remaining_duration(deadline);
                if remaining.is_zero() {
                    return Err(ChannelError::new("stream collect_list deadline expired"));
                }
                let wait = remaining.min(STREAM_COLLECT_POLL_INTERVAL);
                let (next, _) = external
                    .ready
                    .wait_timeout(state, wait)
                    .expect("external stream condvar wait poisoned");
                state = next;
            }
        }
    }
}

fn collect_push<T>(values: &mut Vec<T>, value: T, max_items: usize) -> Result<(), ChannelError> {
    if values.len() >= max_items {
        return Err(ChannelError::new(
            "stream collect_list item budget exhausted",
        ));
    }
    values.push(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_runtime::Executor;

    #[test]
    fn bounded_rejects_non_positive_capacity() {
        assert!(channel_bounded::<i64>(0).is_err());
        assert!(channel_bounded::<i64>(-1).is_err());
        assert!(channel_bounded::<i64>(1).is_ok());
    }

    #[test]
    fn bounded_rejects_capacity_above_runtime_ceiling() {
        let error = channel_bounded::<i64>(1_000_001)
            .err()
            .expect("oversized capacity should fail");
        assert!(channel_error_message(&error).contains("runtime ceiling"));
        assert!(channel_bounded::<i64>(i64::MAX).is_err());
    }

    #[test]
    fn external_stream_wakes_when_a_background_sender_produces() {
        let (sender, receiver) = std_mpsc::channel();
        let stream = stream_from_external_receiver(receiver);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            sender
                .send(Ok(7_i64))
                .expect("stream receiver should remain alive");
        });

        let value = Executor::new()
            .run_pending(stream_next(&stream))
            .expect("external stream should not fail");
        assert_eq!(value, Some(7));
    }

    #[test]
    fn external_stream_bridge_is_bounded_and_drop_cancellable() {
        let (sender, receiver) = std_mpsc::sync_channel(EXTERNAL_STREAM_CAPACITY * 2);
        let stream = stream_from_external_receiver(receiver);
        let producer = std::thread::spawn(move || {
            for value in 0..EXTERNAL_STREAM_CAPACITY * 4 {
                if sender.send(Ok(value)).is_err() {
                    return;
                }
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        let queued = match &*stream.backend.borrow() {
            RssStreamBackend::External(external) => external
                .state
                .lock()
                .expect("external stream lock should not be poisoned")
                .items
                .len(),
            _ => panic!("expected external stream backend"),
        };
        assert_eq!(queued, EXTERNAL_STREAM_CAPACITY);

        drop(stream);
        producer
            .join()
            .expect("dropping stream should unblock the source producer");
    }

    #[test]
    fn cancellable_receive_wakes_and_returns_an_error() {
        let mut channel = channel_bounded::<i64>(1).expect("channel should be created");
        let _sender = channel_sender(&channel);
        let receiver = channel_receiver(&mut channel).expect("receiver should be created");
        let mut source = crate::cancellation_source_new();
        let token = crate::cancellation_source_token(&source);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            crate::cancellation_source_cancel(&mut source);
        });

        let error = Executor::new()
            .run_pending(receiver_recv_cancellable(&receiver, &token))
            .expect_err("cancellation should complete the pending receive");
        assert_eq!(channel_error_message(&error), "channel recv cancelled");
    }

    #[test]
    fn receiver_can_only_be_taken_once() {
        let mut channel = channel_bounded::<i64>(4).unwrap();
        assert!(channel_receiver(&mut channel).is_ok());
        assert!(channel_receiver(&mut channel).is_err());
    }

    #[test]
    fn send_after_close_fails() {
        let channel = channel_bounded::<i64>(4).unwrap();
        let mut sender = channel_sender(&channel);
        sender_close(&mut sender);
        let mut pending = sender_send(&sender, 7);
        let mut executor = Executor::new();
        match executor.poll_once(&mut pending) {
            AsyncPoll::Ready(Err(_)) => {}
            AsyncPoll::Ready(Ok(())) => panic!("send after close should not enqueue"),
            AsyncPoll::Pending => panic!("send after close should fail immediately"),
        }
    }

    #[test]
    fn dropping_sender_releases_its_slot() {
        // Two producers; dropping one then closing the other drains the receiver
        // to end-of-stream (`None`) — proving `Drop` decremented the count.
        let mut channel = channel_bounded::<i64>(4).unwrap();
        let sender_a = channel_sender(&channel);
        let mut sender_b = sender_a.clone();
        let receiver = channel_receiver(&mut channel).unwrap();
        let mut executor = Executor::new();

        drop(sender_a);
        sender_close(&mut sender_b);

        let mut recv = receiver_recv(&receiver);
        match executor.poll_once(&mut recv) {
            AsyncPoll::Ready(Ok(None)) => {}
            AsyncPoll::Ready(Ok(Some(_))) => panic!("channel should be empty"),
            AsyncPoll::Ready(Err(_)) => panic!("recv errored"),
            AsyncPoll::Pending => panic!("expected end-of-stream after all senders gone"),
        }
    }

    #[test]
    fn producer_consumer_interleave_without_deadlock() {
        // One producer sends 1..=5 into a capacity-2 channel; the consumer
        // drains it. Sends block when the channel is full, so this only
        // completes if the poll loop interleaves the two tasks.
        let mut channel = channel_bounded::<i64>(2).unwrap();
        let mut sender = channel_sender(&channel);
        let receiver = channel_receiver(&mut channel).unwrap();
        let mut executor = Executor::new();

        let mut to_send: VecDeque<i64> = (1..=5).collect();
        let mut current_send: Option<SendPending<i64>> = None;
        let mut sender_closed = false;
        let mut current_recv: Option<RecvPending<i64>> = None;
        let mut received = Vec::new();
        let mut done = false;

        while !done {
            let mut progress = false;

            if !sender_closed {
                if current_send.is_none() {
                    match to_send.pop_front() {
                        Some(value) => current_send = Some(sender_send(&sender, value)),
                        None => {
                            sender_close(&mut sender);
                            sender_closed = true;
                            progress = true;
                        }
                    }
                }
                if let Some(pending) = &mut current_send
                    && let AsyncPoll::Ready(result) = executor.poll_once(pending)
                {
                    result.unwrap();
                    current_send = None;
                    progress = true;
                }
            }

            if current_recv.is_none() {
                current_recv = Some(receiver_recv(&receiver));
            }
            match executor.poll_once(current_recv.as_mut().unwrap()) {
                AsyncPoll::Ready(Ok(Some(value))) => {
                    received.push(value);
                    current_recv = None;
                    progress = true;
                }
                AsyncPoll::Ready(Ok(None)) => done = true,
                AsyncPoll::Ready(Err(error)) => panic!("recv failed: {error:?}"),
                AsyncPoll::Pending => {}
            }

            if !progress {
                executor.yield_once();
            }
        }

        assert_eq!(received, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn stream_next_reads_from_receiver_until_closed() {
        let mut channel = channel_bounded::<i64>(2).unwrap();
        let mut sender = channel_sender(&channel);
        let receiver = channel_receiver(&mut channel).unwrap();
        let stream = receiver_into_stream(receiver);
        let mut executor = Executor::new();

        executor.run_pending(sender_send(&sender, 11)).unwrap();
        sender_close(&mut sender);

        assert_eq!(
            executor.run_pending(stream_next(&stream)).unwrap(),
            Some(11)
        );
        assert_eq!(executor.run_pending(stream_next(&stream)).unwrap(), None);
    }

    #[test]
    fn stream_from_list_and_collect_round_trip() {
        let stream = stream_from_list(vec![1_i64, 2, 3]);
        let mut executor = Executor::new();

        assert_eq!(executor.run_pending(stream_next(&stream)).unwrap(), Some(1));
        assert_eq!(stream_collect_list(&stream).unwrap(), vec![2, 3]);
    }

    #[test]
    fn stream_collect_enforces_item_budget() {
        let stream = stream_from_list(vec![1_i64, 2, 3]);
        let error = stream_collect_list_with_limits(
            &stream,
            2,
            &cancellation_never(),
            &deadline_after_ms(1_000),
        )
        .expect_err("third item should exceed the budget");
        assert!(channel_error_message(&error).contains("item budget"));
    }

    #[test]
    fn stream_collect_observes_cancellation() {
        let (_sender, receiver) = std_mpsc::sync_channel::<Result<i64, ChannelError>>(1);
        let stream = stream_from_external_receiver(receiver);
        let mut source = crate::cancellation_source_new();
        let token = crate::cancellation_source_token(&source);
        crate::cancellation_source_cancel(&mut source);
        let error = stream_collect_list_with_limits(&stream, 10, &token, &deadline_after_ms(1_000))
            .expect_err("cancelled collection should stop");
        assert!(channel_error_message(&error).contains("cancelled"));
    }
}
