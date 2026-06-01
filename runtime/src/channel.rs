//! Bounded MPSC channel — a cooperative, single-isolate producer/consumer
//! primitive. `send`/`recv` are pendings that yield (return `Pending`) when the
//! channel is full/empty; the enclosing `task_group` poll loop then drives the
//! sibling task, so a full channel never deadlocks. No `Send` bound and no Tokio:
//! the shared state is `Rc<RefCell<..>>`, matching RSScript's single isolate.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use crate::async_runtime::{AsyncPoll, Context, Pending, RssCancellationToken};

/// Opaque channel error surfaced to RSScript. (Variant matching — `Closed` vs
/// `Cancelled` vs `InvalidCapacity` — is deferred; for now the message
/// distinguishes them and `?` propagation is the main pattern.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelError {
    message: String,
}

impl ChannelError {
    fn new(message: &str) -> Self {
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
    receiver: RssReceiver<T>,
}

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
    if capacity <= 0 {
        return Err(ChannelError::new("channel capacity must be positive"));
    }
    Ok(RssChannel {
        state: Rc::new(RefCell::new(ChannelState {
            capacity: capacity as usize,
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
    RssStream { receiver }
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
    fn poll(&mut self, _cx: &mut Context<'_>) -> AsyncPoll<Result<(), ChannelError>> {
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

pub fn stream_next<T>(stream: &RssStream<T>) -> RecvPending<T> {
    receiver_recv(&stream.receiver)
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
    fn poll(&mut self, _cx: &mut Context<'_>) -> AsyncPoll<Result<Option<T>, ChannelError>> {
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
        AsyncPoll::Pending
    }
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
                if let Some(pending) = &mut current_send {
                    if let AsyncPoll::Ready(result) = executor.poll_once(pending) {
                        result.unwrap();
                        current_send = None;
                        progress = true;
                    }
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
}
