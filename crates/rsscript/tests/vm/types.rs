//! Spec §6 — register-VM execution: structs, sums, options, results
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_spawned_producer_consumer_channel_like_backend() {
    // A spawned producer fills a capacity-1 channel while the consumer drains it:
    // exercises both recv-on-empty and send-on-full parking plus cross-task wakeups
    // (the producer's second send blocks until the consumer's first recv frees a slot).
    let source = r#"
features: native, local, async

async fn produce(sender: read Sender<Int>) -> Result<Unit, ChannelError> {
    local a = 10
    await Sender.send<Int>(sender: read sender, value: take a)?
    local b = 20
    await Sender.send<Int>(sender: read sender, value: take b)?
    return Ok(Unit)
}

async fn drain(receiver: read Receiver<Int>) -> Result<Int, ChannelError> {
    let first = await Receiver.recv<Int>(receiver: read receiver)?
    let second = await Receiver.recv<Int>(receiver: read receiver)?
    let a = match first {
        Some(value) => value
        None => 0
    }
    let b = match second {
        Some(value) => value
        None => 0
    }
    return Ok(a + b)
}

async fn main() -> Result<Unit, ChannelError> {
    match Channel.bounded<Int>(capacity: 1) {
        Ok(channel) => {
            let sender = Channel.sender<Int>(channel: read channel)
            local channel_value = channel
            match Channel.receiver<Int>(channel: mut channel_value) {
                Ok(receiver) => {
                    task_group {
                        async let producer = produce(sender: read sender)
                        async let consumer = drain(receiver: read receiver)
                        await producer?
                        let total = await consumer?
                        Log.write(message: read String.from_int(value: total))
                    }
                }
                Err(error) => {
                    Log.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Log.write(message: read ChannelError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-producer-consumer.rss", source, []);
}

#[test]
fn compiled_backend_accepts_inline_read_copy_expression_args() {
    let source = r#"
features: local

fn main() -> Unit {
    local xs = List.new<Int>()
    List.push<Int>(list: mut xs, value: read (0 - 1))
    Log.write(message: read String.from_int(value: List.get<Int>(list: read xs, index: 0)))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-inline-read-copy-expr.rss", source, []);
}

#[test]
fn reg_vm_runs_array_literal_and_index_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let values: List<Int> = [2, 4, 6, 8]
    let total = values[0] + values[1] + values[2] + values[3]
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-array-index.rss", source, []);
}

#[test]
fn reg_vm_runs_managed_struct_value_like_interpreter() {
    let source = r#"
features: local

struct Box {
    value: Int
}

fn main() -> Unit {
    local box = Box(value: 7)
    let managed = manage box
    Log.write(message: read String.from_int(value: managed.value))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-managed-struct.rss", source, []);
}

#[test]
fn reg_vm_runs_capability_from_construction_like_compiled_backend() {
    let source = r#"
features: local

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Log.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn main() -> Unit {
    local writer = BufferWriter(count: 1)
    local cap: Capability<Writer> = Capability<Writer>.from(value: take writer)
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-capability-from.rss", source, []);
}

#[test]
fn reg_vm_runs_index_assignment_like_interpreter() {
    let source = r#"
fn main() -> Unit {
    let mut values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)
    values[2] = 30
    Log.write(message: read String.from_int(value: values[2]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-gap-index-assignment.rss", source, []);
}
