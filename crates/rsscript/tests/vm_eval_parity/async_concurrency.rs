//! eval≡lowered parity: async, streams, channels
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_cancellation_intrinsics() {
    let source = r#"

fn main() -> Unit {
    local source = CancellationSource.new()
    let token = CancellationSource.token(source: read source)
    if !CancellationToken.is_cancelled(token: read token) {
        Output.write(message: read "not-cancelled")
    }

    CancellationSource.cancel(source: mut source)
    if CancellationToken.is_cancelled(token: read token) {
        Output.write(message: read "cancelled")
    }

    let second = CancellationSource.token(source: read source)
    if CancellationToken.is_cancelled(token: read second) {
        Output.write(message: read "second-cancelled")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-cancellation.rss",
        "rsscript_parity_cancellation",
        source,
    );
}

#[test]
fn parity_channel_sync_intrinsics() {
    let source = r#"

async fn main() -> Result<Unit, ChannelError> {
    match Channel.bounded<Int>(capacity: 0) {
        Ok(channel) => {
            let sender: Sender<Int> = Channel.sender<Int>(channel: read channel)
            let _ = sender
            Output.write(message: read "unexpected-channel")
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }

    let mut channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let mut sender: Sender<Int> = Channel.sender<Int>(channel: read channel)
    Sender.close<Int>(sender: mut sender)
    Output.write(message: read "sender-closed")
    let mut receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut channel)?
    Receiver.close<Int>(receiver: mut receiver)
    Output.write(message: read "receiver-closed")
    match Channel.receiver<Int>(channel: mut channel) {
        Ok(receiver) => {
            let _ = receiver
            Output.write(message: read "unexpected-receiver")
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }

    local items = List<String>.new()
    List.push<String>(list: mut items, value: read "one")
    List.push<String>(list: mut items, value: read "two")
    let stream: Stream<String> = Stream.from_list<String>(items: take items)
    let collected = Stream.collect_list<String>(stream: read stream)?
    Output.write(message: read List.join<String>(list: read collected, separator: read ","))

    let mut empty_channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let mut empty_sender: Sender<Int> = Channel.sender<Int>(channel: read empty_channel)
    Sender.close<Int>(sender: mut empty_sender)
    local empty_receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut empty_channel)?
    let empty_stream: Stream<Int> = Receiver.into_stream<Int>(receiver: take empty_receiver)
    let empty_items = Stream.collect_list<Int>(stream: read empty_stream)?
    Output.write(message: read String.from_int(value: List.len<Int>(list: read empty_items)))

    let mut data_channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let mut data_sender: Sender<Int> = Channel.sender<Int>(channel: read data_channel)
    let data_receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut data_channel)?
    local first = 10

    let mut none_channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let mut none_sender: Sender<Int> = Channel.sender<Int>(channel: read none_channel)
    let none_receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut none_channel)?
    Sender.close<Int>(sender: mut none_sender)

    let mut cancelled_send_channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let cancelled_send_sender: Sender<Int> = Channel.sender<Int>(channel: read cancelled_send_channel)
    local cancelled_send_source = CancellationSource.new()
    let cancelled_send_token = CancellationSource.token(source: read cancelled_send_source)
    CancellationSource.cancel(source: mut cancelled_send_source)
    local cancelled_value = 30

    let mut cancelled_recv_channel: Channel<Int> = Channel.bounded<Int>(capacity: 1)?
    let cancelled_recv_receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut cancelled_recv_channel)?
    local cancelled_recv_source = CancellationSource.new()
    let cancelled_recv_token = CancellationSource.token(source: read cancelled_recv_source)
    CancellationSource.cancel(source: mut cancelled_recv_source)

    local next_items = List<Int>.new()
    List.push<Int>(list: mut next_items, value: read 41)
    let next_stream: Stream<Int> = Stream.from_list<Int>(items: take next_items)

    local empty_next_items = List<Int>.new()
    let empty_next_stream: Stream<Int> = Stream.from_list<Int>(items: take empty_next_items)

    await Sender.send<Int>(sender: read data_sender, value: take first)?
    Sender.close<Int>(sender: mut data_sender)
    match await Receiver.recv<Int>(receiver: read data_receiver)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "recv-none")
        }
    }
    match await Receiver.recv<Int>(receiver: read none_receiver)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "recv-none")
        }
    }

    match await Sender.send_cancellable<Int>(sender: read cancelled_send_sender, value: take cancelled_value, token: read cancelled_send_token) {
        Ok(_) => {
            Output.write(message: read "unexpected-send")
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }
    match await Receiver.recv_cancellable<Int>(receiver: read cancelled_recv_receiver, token: read cancelled_recv_token) {
        Ok(_) => {
            Output.write(message: read "unexpected-recv")
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
        }
    }
    match await Stream.next<Int>(stream: read next_stream)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "stream-none")
        }
    }
    match await Stream.next<Int>(stream: read empty_next_stream)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "stream-none")
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-channel-sync.rss",
        "rsscript_parity_channel_sync",
        source,
        &[],
        &[],
    );
}

#[test]
fn parity_await_in_expression() {
    // `await` nested inside argument, return, and assignment-target expressions
    // is hoisted to preceding `let` bindings. Evaluation order (left-to-right)
    // and the doubly-nested `await g(await f())` form must match across backends.
    let source = r#"

async fn step(n: Int) -> Result<Int, String> {
    Output.write(message: read String.from_int(value: n))
    return Ok(n)
}

fn add(a: Int, b: Int) -> Int { return a + b }

async fn main() -> Result<Unit, String> {
    let total = add(a: await step(n: 1)?, b: await step(n: 2)?)
    let nested = await step(n: await step(n: 3)?)?
    let mut xs = [0, 0, 0]
    xs[await step(n: 0)?] = total + nested
    Output.write(message: read String.from_int(value: xs[0]))
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-await-in-expression.rss",
        "rsscript_parity_await_in_expression",
        source,
    );
}

#[test]
fn parity_message_channel_roundtrip() {
    // Cross-isolate message channel (spec §20.2-3): `Channel.message<T>` requires a
    // cross-isolate-transferable payload and reuses the bounded-channel runtime, so
    // send/recv must behave identically across backends.
    let source = r#"

async fn main() -> Result<Unit, ChannelError> {
    let mut channel: Channel<Int> = Channel.message<Int>(capacity: 1)?
    let mut sender: Sender<Int> = Channel.sender<Int>(channel: read channel)
    let receiver: Receiver<Int> = Channel.receiver<Int>(channel: mut channel)?
    local first = 7
    await Sender.send<Int>(sender: read sender, value: take first)?
    Sender.close<Int>(sender: mut sender)
    match await Receiver.recv<Int>(receiver: read receiver)? {
        Some(value) => {
            Output.write(message: read String.from_int(value: value))
        }
        None => {
            Output.write(message: read "recv-none")
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend_with_distinct_args_allowing_unused_mut_warning(
        "parity-message-channel.rss",
        "rsscript_parity_message_channel",
        source,
        &[],
        &[],
    );
}
