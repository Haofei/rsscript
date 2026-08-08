//! Spec §6 — register-VM execution: structs, sums, options, results
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn reg_vm_runs_spawned_producer_consumer_channel_like_backend() {
    // A spawned producer fills a capacity-1 channel while the consumer drains it:
    // exercises both recv-on-empty and send-on-full parking plus cross-task wakeups
    // (the producer's second send blocks until the consumer's first recv frees a slot).
    let source = r#"

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
                        Output.write(message: read String.from_int(value: total))
                    }
                }
                Err(error) => {
                    Output.write(message: read ChannelError.message(error: read error))
                }
            }
        }
        Err(error) => {
            Output.write(message: read ChannelError.message(error: read error))
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

fn main() -> Unit {
    local xs = List.new<Int>()
    List.push<Int>(list: mut xs, value: read (0 - 1))
    Output.write(message: read String.from_int(value: List.get<Int>(list: read xs, index: 0)))
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
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-array-index.rss", source, []);
}

#[test]
fn reg_vm_runs_managed_struct_value_like_interpreter() {
    let source = r#"

struct Box {
    value: Int
}

fn main() -> Unit {
    local box = Box(value: 7)
    let managed = manage box
    Output.write(message: read String.from_int(value: managed.value))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-managed-struct.rss", source, []);
}

#[test]
fn reg_vm_runs_dyn_from_construction_like_compiled_backend() {
    let source = r#"

protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
}

struct BufferWriter {
    count: Int
}

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit {
    Output.write(message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}

fn main() -> Unit {
    local writer = BufferWriter(count: 1)
    local cap: Dyn<Writer> = Dyn<Writer>.from(value: take writer)
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-external_binding-from.rss", source, []);
}

#[test]
fn reg_vm_runs_class_alias_external_binding_like_compiled_backend() {
    let source = r#"
protocol Readable {
    fn get(self: read Self) -> Int
}

class Gauge {
    value: Int
}

type G = Gauge

fn Gauge.get(self: read Gauge) -> Int {
    return self.value
}

impl Readable for Gauge {
    get = Gauge.get
}

fn read_alias(value: read G) -> Int {
    return value.value
}

fn main() -> Unit {
    let gauge: G = Gauge(value: 7)
    Output.write(message: String.from_int(
        value: read_alias(value: read gauge)
    ))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-class-external_binding.rss", source, []);
}

#[test]
fn reg_vm_runs_generic_sum_with_class_payload_like_compiled_backend() {
    let source = r#"
class Node {
    value: Int
}

sum Envelope<T> {
    Value(value: T)
    NodeValue(node: Node)
}

fn main() -> Unit {
    Output.write(message: "generic sum compiled")
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-generic-class-sum.rss", source, []);
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
    Output.write(message: read String.from_int(value: values[2]))
    return Unit
}
"#;

    assert_reg_vm_matches_compiled_backend("reg-vm-gap-index-assignment.rss", source, []);
}
