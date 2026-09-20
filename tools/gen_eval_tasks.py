#!/usr/bin/env python3
"""Generator for the generation tasks in the eval corpus.

Kept in-tree so the task/expected pairs can be regenerated deterministically
instead of being hand-edited out of sync.
"""

from __future__ import annotations

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
EVALS = ROOT / "evals"

AUTOMATION = "interfaces/automation_api.rssi"
CONFIG_STORE = "interfaces/config_store.rssi"
REPORT_SINK = "interfaces/report_sink.rssi"
DEVICE_SESSION = "interfaces/device_session.rssi"
INVENTORY = "interfaces/inventory_api.rssi"

TASKS = [
    {
        "id": "json-config-validate",
        "title": "Parse and validate a JSON service config",
        "prompt": (
            "Write a single RSScript file that parses a JSON service configuration and "
            "validates it. Declare a struct ServiceConfig with fields name: String, "
            "port: Int, and enabled: Bool. Write fn load_config(text: String) -> "
            "Result<fresh ServiceConfig, String> that parses the text with the core Json "
            "interface, reads the string field `name` and the integer field `port`, and "
            "rejects a port below 1 or above 65535 with an explanatory Err. Handle every "
            "JsonError explicitly; do not unwrap. Add fn main() -> Unit that calls "
            "load_config on a literal config and writes either the name or the error with "
            "Output.write. Use only the core interfaces; do not invent a host API."
        ),
        "interfaces": [],
        "tags": ["json", "validation", "result", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Json.parse"},
            {"kind": "source_contains", "value": "Err("},
            {"kind": "source_excludes", "value": ".unwrap()"},
        ],
    },
    {
        "id": "csv-record-transform",
        "title": "Transform a CSV-like list of records",
        "prompt": (
            "Write a single RSScript file that parses CSV-like text into records. Declare "
            "a struct Record with fields id: String and amount: Int. Write fn "
            "parse_record(line: read String) -> Result<fresh Record, String> that splits "
            "the line on a comma, trims both fields, requires exactly two fields, and "
            "returns Err when the amount is not an integer. Write fn parse_all(text: read "
            "String) -> Result<fresh List<Record>, String> that skips blank lines and "
            "collects every record, and fn total_amount(records: read List<Record>) -> Int "
            "that sums the amounts. Add fn main() -> Unit that parses a literal two-line "
            "document and writes the total or the error. Use only the core interfaces."
        ),
        "interfaces": [],
        "tags": ["collections", "parsing", "result", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "String.split"},
            {"kind": "source_contains", "value": "Err("},
            {"kind": "source_excludes", "value": ".unwrap()"},
        ],
    },
    {
        "id": "retry-bounded",
        "title": "Bounded retry loop over a fallible host call",
        "prompt": (
            "Write a single RSScript file that retries a fallible host call a bounded "
            "number of times. The only host surface available is the declared interface: "
            "Sync.push(id: read String, amount: Int) -> Result<Int, SyncError>, "
            "Sync.is_retryable(error: read SyncError) -> Bool, SyncError.message(error: "
            "read SyncError) -> fresh String, and Metrics.record(name: read String, value: "
            "Int) -> Unit. Write fn push_with_retry(id: read String, amount: Int, "
            "max_attempts: Int) -> Result<Int, String> that attempts the push at most "
            "max_attempts times, stops immediately when the error is not retryable, "
            "records the attempt count with Metrics.record on success, and returns the "
            "last error message otherwise. Add fn main() -> Unit that calls it and writes "
            "the result. Do not invent any other host API."
        ),
        "interfaces": [AUTOMATION],
        "tags": ["result", "retry", "host-boundary", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Sync.push"},
            {"kind": "source_contains", "value": "Sync.is_retryable"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "producer-consumer-bounded",
        "title": "Producer and consumer over a bounded channel with cancellation",
        "prompt": (
            "Write a single RSScript file with a producer and a consumer over a bounded "
            "channel. In main, create a bounded Int channel with capacity 2, take its "
            "sender and receiver, and run the producer and consumer concurrently inside a "
            "task_group. The producer sends four values and must stop early when the "
            "task_group's cancellation token is observed; obtain that token inside the "
            "task_group with Task.cancellation_token() and pass it into the producer, "
            "because an async fn has no enclosing task_group. The consumer receives until "
            "it has seen four values or the channel closes. Await both handles inside the "
            "task_group. Channel values transfer ownership, so send with take."
        ),
        "interfaces": [],
        "tags": ["async", "channel", "cancellation", "ownership", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "task_group"},
            {"kind": "source_contains", "value": "Channel.bounded"},
            {"kind": "source_contains", "value": "Task.cancellation_token()"},
            {"kind": "source_contains", "value": "take "},
        ],
    },
    {
        "id": "resource-scope-host",
        "title": "Copy a setting inside a `with` resource scope",
        "prompt": (
            "Write a single RSScript file that copies one key of a scoped host resource to "
            "another key. The only host surface available is the declared interface: a "
            "resource ConfigStore with ConfigStore.open(path: read String) -> "
            "Result<ConfigStore, String>, ConfigStore.read_key(store: mut ConfigStore, "
            "key: read String) -> Result<fresh String, String>, and "
            "ConfigStore.write_key(store: mut ConfigStore, key: read String, value: read "
            "String) -> Result<Unit, String>. Write fn copy_setting(path: read String, key: "
            "read String, target: read String) -> Result<Unit, String> that acquires the "
            "store with a `with` scope so cleanup runs, reads the key, and writes it to the "
            "target key. The store must not escape the `with` scope. Add fn main() -> Unit "
            "that calls it and reports success or the error."
        ),
        "interfaces": [CONFIG_STORE],
        "tags": ["resource", "with", "host-boundary", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "with ConfigStore.open"},
            {"kind": "source_contains", "value": "store: mut store"},
            {"kind": "diagnostic_absent", "value": "RS0702"},
        ],
    },
    {
        "id": "protocol-dyn-dispatch",
        "title": "Two protocol implementations dispatched through `Dyn<P>`",
        "prompt": (
            "Write a single RSScript file that declares a protocol Formatter with one "
            "method fn format(self: read Self) -> fresh String, and two structs that "
            "implement it: PlainFormatter (field prefix: String) rendering a plain string, "
            "and JsonFormatter (field field: String) rendering a JSON field with the core "
            "Json interface. Provide the implementation functions and an `impl Formatter "
            "for ...` block for each. Write fn render(formatter: read Dyn<Formatter>) -> "
            "fresh String that dispatches dynamically. Add fn main() -> Unit that builds "
            "one value of each struct, converts each into a Dyn<Formatter>, and writes both "
            "rendered strings. Dynamic protocol dispatch is written as Dyn<P>."
        ),
        "interfaces": [],
        "tags": ["protocol", "dyn", "dispatch", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "protocol Formatter"},
            {"kind": "source_contains", "value": "Dyn<Formatter>"},
            {"kind": "source_contains", "value": "Dyn.from"},
        ],
    },
    {
        "id": "generic-struct-bound",
        "title": "Generic container with a `Struct` bound",
        "prompt": (
            "Write a single RSScript file with a generic container. Declare struct "
            "Batch<T: Struct> with fields label: String and items: List<T>, and a struct "
            "Reading with fields sensor: String and value: Int. Write fn make_batch<T: "
            "Struct>(label: take String, items: take List<T>) -> fresh Batch<T> that "
            "constructs the batch, and fn batch_size<T: Struct>(batch: read Batch<T>) -> "
            "Int that returns the item count. Add fn main() -> Unit that builds a "
            "Batch<Reading> with one reading and writes its size. Ownership transfer into "
            "the constructor must be explicit."
        ),
        "interfaces": [],
        "tags": ["generics", "bounds", "ownership", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "T: Struct"},
            {"kind": "source_contains", "value": "take "},
            {"kind": "diagnostic_absent", "value": "RS0308"},
        ],
    },
    {
        "id": "retains-declaration",
        "title": "Declare `retains` for parameters stored past return",
        "prompt": (
            "Write a single RSScript file with a class EventLog holding two List<String> "
            "fields, entries and tags. Write fn EventLog.new() -> EventLog that builds an "
            "empty log, fn EventLog.record(log: mut EventLog, entry: String, tag: String) "
            "-> Unit that pushes the entry and the tag into the corresponding lists, and fn "
            "EventLog.size(log: read EventLog) -> Int. Because record stores both "
            "parameters past return, it must declare the retention contract with one "
            "structured clause per retained parameter. Add fn main() -> Unit that records "
            "two events and writes the size."
        ),
        "interfaces": [],
        "tags": ["retention", "retains", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "retains("},
            {"kind": "diagnostic_absent", "value": "RS0501"},
            {"kind": "diagnostic_absent", "value": "RS0007"},
        ],
    },
    {
        "id": "take-mut-chain",
        "title": "A take/mut ownership chain through three functions",
        "prompt": (
            "Write a single RSScript file that threads one value through three functions "
            "with three different data effects. The declared host interface supplies "
            "struct Report { title: String, lines: List<String> } and ReportSink.emit("
            "report: take Report) -> Unit. Write fn build_report(title: take String) -> "
            "fresh Report that consumes the title, fn add_line(report: mut Report, line: "
            "read String) -> Unit that appends a copy of the line, and fn "
            "finish_report(report: take Report) -> Unit that hands the report to "
            "ReportSink.emit. Add fn main() -> Unit that builds a report, adds two lines, "
            "and finishes it. Call-site `mut` and `take` are explicit; a `read` wrapper is "
            "omitted because it is the default."
        ),
        "interfaces": [REPORT_SINK],
        "tags": ["ownership", "take", "mut", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "take report"},
            {"kind": "source_contains", "value": "mut report"},
            {"kind": "diagnostic_absent", "value": "RS0202"},
        ],
    },
    {
        "id": "task-group-select",
        "title": "Race two receivers with `select` inside a `task_group`",
        "prompt": (
            "Write a single RSScript file that races two bounded channels. Create two "
            "bounded Int channels of capacity 1 and take a sender and a receiver from "
            "each. Inside one task_group, start an async let for each feeding task, then "
            "use a `select` block with one arm per receiver, where each arm awaits "
            "Receiver.recv on that receiver, writes which side won, and cancels an explicit "
            "CancellationSource. Await both async let handles inside the same task_group. "
            "Note that `async let` handles cannot be awaited from inside a `select` arm: "
            "select arms await a direct async operation."
        ),
        "interfaces": [],
        "tags": ["async", "task-group", "select", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "task_group"},
            {"kind": "source_contains", "value": "select {"},
            {"kind": "diagnostic_absent", "value": "RS0015"},
        ],
    },
    {
        "id": "option-defaults-chain",
        "title": "Resolve settings from optional lookups with defaults",
        "prompt": (
            "Write a single RSScript file that resolves settings from a parallel key and "
            "value list. Declare struct Settings with fields region: String and retries: "
            "Int. Write fn lookup(keys: read List<String>, values: read List<String>, key: "
            "read String) -> Option<String> that returns the matching value or None, and fn "
            "settings_from(keys: read List<String>, values: read List<String>) -> fresh "
            "Settings that defaults region to \"us-east\" and retries to 3 when the key is "
            "absent or the retry value does not parse as an integer. Handle every Option "
            "with an explicit match; do not unwrap. Add fn main() -> Unit that writes both "
            "resolved fields."
        ),
        "interfaces": [],
        "tags": ["option", "defaults", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Option<String>"},
            {"kind": "source_contains", "value": "None"},
            {"kind": "source_excludes", "value": ".unwrap()"},
        ],
    },
    {
        "id": "result-error-mapping",
        "title": "Map a host error into a domain sum type",
        "prompt": (
            "Write a single RSScript file that maps host errors into a domain error type. "
            "Declare sum PipelineError with variants BadInput(detail: String) and "
            "Upstream(detail: String), and fn PipelineError.detail(error: read "
            "PipelineError) -> fresh String that renders each variant. Write fn "
            "parse_amount(raw: read String) -> Result<Int, PipelineError> returning "
            "BadInput when the text is not an integer, and fn push_amount(id: read String, "
            "raw: read String) -> Result<Int, PipelineError> that parses the amount and "
            "then calls the declared host function Sync.push(id:, amount:), converting any "
            "SyncError into Upstream using SyncError.message. Add fn main() -> Unit that "
            "writes the accepted count or the rendered error. Do not invent a host API."
        ),
        "interfaces": [AUTOMATION],
        "tags": ["result", "sum-type", "error-mapping", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "sum PipelineError"},
            {"kind": "source_contains", "value": "SyncError.message"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "map-aggregate-counts",
        "title": "Aggregate log-line counts into a Map",
        "prompt": (
            "Write a single RSScript file that counts log lines per level. Write fn "
            "count_by_level(lines: read List<String>) -> fresh Map<String, Int> that takes "
            "the text before the first space as the level (falling back to \"unknown\") and "
            "accumulates a count per level. Write fn render_counts(counts: read Map<String, "
            "Int>) -> fresh String that renders each level and its count joined by commas. "
            "Add fn main() -> Unit that counts three literal lines and writes the rendered "
            "result. Use only the core collection interfaces; mutation of the map must be "
            "explicit at the call site."
        ),
        "interfaces": [],
        "tags": ["collections", "map", "mut", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Map.insert"},
            {"kind": "source_contains", "value": "map: mut"},
            {"kind": "diagnostic_absent", "value": "RS0202"},
        ],
    },
    {
        "id": "sum-type-state-machine",
        "title": "Exhaustive match over a job state sum type",
        "prompt": (
            "Write a single RSScript file modelling a job state machine. Declare sum "
            "JobState with variants Queued, Running(worker: String), Done(code: Int), and "
            "Failed(reason: String). Write fn advance(state: read JobState, worker: read "
            "String) -> fresh JobState that moves Queued to Running, Running to Done with "
            "code 0, and leaves Done and Failed unchanged, and fn describe(state: read "
            "JobState) -> fresh String that renders each variant. Every match must be "
            "exhaustive over all four variants. Add fn main() -> Unit that advances a queued "
            "job twice and writes the description."
        ),
        "interfaces": [],
        "tags": ["sum-type", "match", "exhaustiveness", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "sum JobState"},
            {"kind": "diagnostic_absent", "value": "RS0021"},
            {"kind": "diagnostic_absent", "value": "RS0037"},
        ],
    },
    {
        "id": "noescape-filter-callback",
        "title": "Forward a `noescape` predicate to a collection filter",
        "prompt": (
            "Write a single RSScript file that filters alerts through a caller-supplied "
            "predicate. Declare struct Alert with fields level: String and message: String. "
            "Write fn select_alerts(alerts: read List<Alert>, keep: noescape Fn(Alert) -> "
            "Bool) -> fresh List<Alert> that forwards the predicate to the core list filter, "
            "and fn urgent_only(alerts: read List<Alert>) -> fresh List<Alert> that calls it "
            "with a closure keeping only alerts whose level is \"critical\". Add fn main() "
            "-> Unit that builds two alerts and writes the number of urgent ones. The "
            "callback must not escape its call."
        ),
        "interfaces": [],
        "tags": ["closures", "noescape", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "noescape Fn"},
            {"kind": "diagnostic_absent", "value": "RS0802"},
            {"kind": "diagnostic_absent", "value": "RS0803"},
        ],
    },
    {
        "id": "async-stream-consume",
        "title": "Drain a bounded number of items from a Stream",
        "prompt": (
            "Write a single RSScript file that drains a stream. Write async fn drain("
            "stream: Stream<Int>, limit: Int) -> Result<Int, ChannelError> that awaits "
            "Stream.next until it has consumed `limit` items or the stream returns None, "
            "summing the values. In fn main() -> Result<Unit, ChannelError>, build a local "
            "List<Int> with two values, turn it into a Stream with the core interface, and "
            "run drain inside a task_group with an async let that is awaited in the same "
            "task_group; write the total. `await` appears only inside an async function."
        ),
        "interfaces": [],
        "tags": ["async", "stream", "task-group", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Stream.next"},
            {"kind": "source_contains", "value": "task_group"},
            {"kind": "diagnostic_absent", "value": "RS0029"},
        ],
    },
    {
        "id": "list-try-fold-validate",
        "title": "Validate and total a list with a fallible fold",
        "prompt": (
            "Write a single RSScript file that validates and totals a list of numeric "
            "strings. Declare struct Tally with fields total: Int and count: Int. Write fn "
            "add_amount(tally: Tally, raw: String) -> Result<fresh Tally, String> that "
            "rejects a non-integer and a negative amount with an explanatory Err, and fn "
            "tally_all(raws: read List<String>) -> Result<fresh Tally, String> that folds "
            "the list with the core fallible fold so the first failure short-circuits. Add "
            "fn main() -> Unit that tallies three literal amounts and writes the total or "
            "the error."
        ),
        "interfaces": [],
        "tags": ["collections", "fold", "result", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "List.try_fold"},
            {"kind": "source_contains", "value": "Err("},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "host-interface-only",
        "title": "Publish a batch using only the declared host interface",
        "prompt": (
            "Write a single RSScript file that publishes a batch of records. The ONLY host "
            "functions available are the declared interface: Sync.push(id: read String, "
            "amount: Int) -> Result<Int, SyncError>, SyncError.message(error: read "
            "SyncError) -> fresh String, and Metrics.record(name: read String, value: Int) "
            "-> Unit. Write fn publish_batch(ids: read List<String>, amount: Int) -> "
            "Result<Int, String> that pushes each id with the given amount, stops at the "
            "first failure and returns its message, and otherwise records the accepted "
            "total with Metrics.record and returns it. Add fn main() -> Unit that publishes "
            "two ids and writes the outcome. Do not invent, import, or assume any other "
            "host, filesystem, network, logging, time, or environment API."
        ),
        "interfaces": [AUTOMATION],
        "tags": ["host-boundary", "external-symbol", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Sync.push"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
            {"kind": "diagnostic_absent", "value": "RS0024"},
        ],
    },
    {
        "id": "nested-json-array-sum",
        "title": "Sum a field across a nested JSON array",
        "prompt": (
            "Write a single RSScript file that sums a field across a nested JSON array. "
            "Write fn sum_quantities(text: read String) -> Result<Int, JsonError> that "
            "parses the text, reads the object field `items`, iterates the array, reads the "
            "integer field `quantity` from each element, and returns the total. Propagate "
            "every JsonError rather than unwrapping. Add fn main() -> Unit that calls it on "
            "a literal document containing two items and writes the total or the error "
            "message. Use only the core Json interface."
        ),
        "interfaces": [],
        "tags": ["json", "result", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Json.parse"},
            {"kind": "source_excludes", "value": ".unwrap()"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "resource-across-await",
        "title": "Keep a resource scope off the await path",
        "prompt": (
            "Write a single RSScript file that reads a cursor from a scoped host resource "
            "and then fetches a page asynchronously. The declared host interfaces supply "
            "the resource ConfigStore (open/read_key/write_key) and the async function "
            "Sync.fetch_page(cursor: read String) -> Result<fresh String, SyncError>, plus "
            "SyncError.message. Write fn read_cursor(path: read String) -> Result<fresh "
            "String, String> that acquires the store with a `with` scope and reads the key "
            "\"cursor\", and async fn sync_next_page(path: read String) -> Result<String, "
            "String> that gets the cursor and then awaits the fetch. Do not hold the "
            "resource scope across the await. In fn main() -> Result<Unit, String>, run "
            "sync_next_page inside a task_group with an async let awaited in the same "
            "task_group, and write the page."
        ),
        "interfaces": [CONFIG_STORE, AUTOMATION],
        "tags": ["resource", "async", "await", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "with ConfigStore.open"},
            {"kind": "diagnostic_absent", "value": "RS0702"},
            {"kind": "diagnostic_absent", "value": "RS0031"},
        ],
    },
    {
        "id": "device-report-render",
        "title": "Render a list of device readings as a report string",
        "prompt": (
            "Write a single RSScript file that renders a list of records as one report "
            "string. Declare a struct Reading with fields sensor: String and value: Int. "
            "Write fn format_reading(reading: read Reading) -> fresh String that renders "
            "one reading as `sensor=value`, and fn render_report(title: read String, "
            "readings: read List<Reading>) -> fresh String that builds a list of lines "
            "holding the title, one line per reading, and a final `count=N` line, then "
            "joins the lines with the core list join and a newline separator. Add fn "
            "main() -> Unit that builds two readings and writes the report with "
            "Output.write. Use only the core interfaces; do not invent a host API."
        ),
        "interfaces": [],
        "tags": ["collections", "strings", "report", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "List.join"},
            {"kind": "source_excludes", "value": ".unwrap()"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "config-typed-errors",
        "title": "Validate a config and return a typed error",
        "prompt": (
            "Write a single RSScript file that validates a configuration and reports "
            "failures as a typed error rather than a string. Declare struct PumpConfig "
            "with fields name: String, interval_ms: Int and retries: Int, and sum "
            "ConfigError with variants MissingField(field: String) and OutOfRange(field: "
            "String, value: Int). Write fn ConfigError.describe(error: read ConfigError) "
            "-> fresh String rendering each variant, and fn validate(config: read "
            "PumpConfig) -> Result<Unit, ConfigError> that rejects an empty name, an "
            "interval_ms outside 100..=60000, and a retries count outside 0..=10. Every "
            "match must cover both variants. Add fn main() -> Unit that validates a "
            "literal config and writes the name or the described error."
        ),
        "interfaces": [],
        "tags": ["validation", "sum-type", "result", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "sum ConfigError"},
            {"kind": "diagnostic_absent", "value": "RS0021"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "firmware-retry-attempts",
        "title": "Retry a declared host call within an attempt budget",
        "prompt": (
            "Write a single RSScript file that retries one fallible host call within an "
            "attempt budget. The only host surface available is the declared interface: "
            "Sync.push(id: read String, amount: Int) -> Result<Int, SyncError>, "
            "Sync.is_retryable(error: read SyncError) -> Bool, SyncError.message(error: "
            "read SyncError) -> fresh String, and Metrics.record(name: read String, "
            "value: Int) -> Unit. Write fn push_until_accepted(id: read String, amount: "
            "Int, budget: Int) -> Result<Int, String> that calls the push at most `budget` "
            "times, returns immediately when the error is not retryable, records the "
            "attempt number with Metrics.record on success, and otherwise returns the last "
            "error message. Add fn main() -> Unit that calls it and writes the outcome. Do "
            "not invent any other host API."
        ),
        "interfaces": [AUTOMATION],
        "tags": ["result", "retry", "host-boundary", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Sync.push"},
            {"kind": "source_contains", "value": "Metrics.record"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "telemetry-cancel-pipeline",
        "title": "Producer and consumer over a channel with an explicit cancellation token",
        "prompt": (
            "Write a single RSScript file that moves telemetry samples from a producer to "
            "a consumer over a bounded channel and cancels both with one token. In main, "
            "create a bounded Int channel with capacity 2, take its sender and receiver, "
            "and create a CancellationSource whose token is passed into both tasks. Write "
            "async fn publish(sender: Sender<Int>, samples: read List<Int>, token: "
            "CancellationToken) -> Result<Int, ChannelError> that stops early when the "
            "token is cancelled and otherwise sends every sample with the cancellable send, "
            "and async fn collect(receiver: Receiver<Int>, limit: Int, token: "
            "CancellationToken) -> Result<Int, ChannelError> that sums received values "
            "until it has seen `limit` of them or the channel closes. Run both inside one "
            "task_group with async let and await both there; cancel the source after the "
            "consumer finishes. Channel values transfer ownership, so send with take."
        ),
        "interfaces": [],
        "tags": ["async", "channel", "cancellation", "ownership", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "task_group"},
            {"kind": "source_contains", "value": "CancellationSource"},
            {"kind": "source_contains", "value": "take "},
        ],
    },
    {
        "id": "session-with-cleanup",
        "title": "Apply settings in a `with` scope that cleans up on the error path",
        "prompt": (
            "Write a single RSScript file that applies a list of settings to a scoped host "
            "session. The only host surface available is the declared interface: a resource "
            "DeviceSession with DeviceSession.open(device: read String) -> "
            "Result<DeviceSession, String>, DeviceSession.apply(session: mut DeviceSession, "
            "setting: read String, value: Int) -> Result<Unit, String>, "
            "DeviceSession.commit(session: mut DeviceSession) -> Result<Int, String>, plus "
            "Audit.note(event: read String) -> Unit. Write fn apply_settings(device: read "
            "String, settings: read List<String>, value: Int) -> Result<Int, String> that "
            "acquires the session with a `with` scope so cleanup runs on every path, "
            "applies each setting in order, and on the first failure notes the aborted "
            "setting with Audit.note and returns that error; on success it commits, notes "
            "the commit, and returns the version. The session must not escape the `with` "
            "scope. Add fn main() -> Unit that calls it and writes the outcome."
        ),
        "interfaces": [DEVICE_SESSION],
        "tags": ["resource", "with", "host-boundary", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "with DeviceSession.open"},
            {"kind": "source_contains", "value": "session: mut session"},
            {"kind": "diagnostic_absent", "value": "RS0702"},
        ],
    },
    {
        "id": "protocol-select-impl",
        "title": "Pick one of two protocol implementations from an input string",
        "prompt": (
            "Write a single RSScript file that picks a sink implementation at run time. "
            "Declare a protocol Sink with one method fn deliver(self: read Self, line: read "
            "String) -> fresh String, and two structs implementing it: ConsoleSink (field "
            "prefix: String) rendering `prefix: line`, and JsonSink (field channel: String) "
            "rendering a JSON field with the core Json interface. Provide the implementation "
            "functions and an `impl Sink for ...` block for each. Write fn sink_for(kind: "
            "read String) -> fresh Dyn<Sink> that returns the JSON sink when kind is "
            '"json" and the console sink otherwise, and fn deliver_line(kind: read String, '
            "line: read String) -> fresh String that dispatches dynamically through it. Add "
            "fn main() -> Unit that writes both renderings. Dynamic protocol dispatch is "
            "written as Dyn<P>."
        ),
        "interfaces": [],
        "tags": ["protocol", "dyn", "dispatch", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "protocol Sink"},
            {"kind": "source_contains", "value": "Dyn<Sink>"},
            {"kind": "source_contains", "value": "Dyn.from"},
        ],
    },
    {
        "id": "callback-helper-twice",
        "title": "Call one `noescape Fn` helper twice with different closures",
        "prompt": (
            "Write a single RSScript file with one callback-taking helper used twice. Write "
            "fn count_matching(values: read List<Int>, predicate: noescape Fn(Int) -> Bool) "
            "-> Int that counts the values the predicate accepts, and fn summarise(values: "
            "read List<Int>, limit: Int) -> fresh String that calls count_matching twice — "
            "once with a closure keeping values above the limit and once with a closure "
            "keeping the rest — and renders the two counts as `high/low`. Add fn main() -> "
            "Unit that writes the summary for a literal list. The callback must not escape "
            "its call."
        ),
        "interfaces": [],
        "tags": ["closures", "noescape", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "noescape Fn"},
            {"kind": "diagnostic_absent", "value": "RS0802"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "closure-capture-local",
        "title": "Call a local closure that captures a surrounding local",
        "prompt": (
            "Write a single RSScript file whose helper builds a closure over its own "
            "locals. Write fn scaled_total(values: read List<Int>, factor: Int, offset: "
            "Int) -> Int that binds a closure capturing factor and offset implicitly, then "
            "calls it once per value and sums the results. A closure that will be called by "
            "name is bound with `local`; it must not be returned or stored. Add fn main() "
            "-> Unit that writes the scaled total of a literal list."
        ),
        "interfaces": [],
        "tags": ["closures", "captures", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "local "},
            {"kind": "diagnostic_absent", "value": "RS0803"},
            {"kind": "diagnostic_absent", "value": "RS0026"},
        ],
    },
    {
        "id": "variant-payload-match",
        "title": "Match a sum type whose variants carry payloads",
        "prompt": (
            "Write a single RSScript file modelling a command union. Declare sum Command "
            "with variants Ping, SetPoint(sensor: String, target: Int) and Shutdown(reason: "
            "String). Write fn apply_command(command: read Command, current: Int) -> Int "
            "that returns the current value for Ping, the target for SetPoint and 0 for "
            "Shutdown, and fn describe(command: read Command) -> fresh String that renders "
            "each variant. Bind the payloads in the patterns; every match must cover all "
            "three variants and bind exactly the fields each variant declares. Add fn "
            "main() -> Unit that builds a SetPoint command and writes both results."
        ),
        "interfaces": [],
        "tags": ["sum-type", "match", "patterns", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "sum Command"},
            {"kind": "diagnostic_absent", "value": "RS0021"},
            {"kind": "diagnostic_absent", "value": "RS0037"},
        ],
    },
    {
        "id": "string-builder-report",
        "title": "Build a multi-line report with `StringBuilder`",
        "prompt": (
            "Write a single RSScript file that builds a report incrementally rather than by "
            "repeated concatenation. Declare struct Entry with fields label: String and "
            "count: Int. Write fn render_entries(title: read String, entries: read "
            "List<Entry>) -> fresh String that uses the core StringBuilder: create one, push "
            "the title and a newline, push an indented `label: count` line per entry, and "
            "finish the builder to produce the string. Finishing consumes the builder, so it "
            "has to be a binding that can be taken. Add fn main() -> Unit that builds two "
            "entries and writes the rendered report."
        ),
        "interfaces": [],
        "tags": ["strings", "string-builder", "ownership", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "StringBuilder.new"},
            {"kind": "source_contains", "value": "StringBuilder.finish"},
            {"kind": "diagnostic_absent", "value": "RS0308"},
        ],
    },
    {
        "id": "map-default-aggregate",
        "title": "Aggregate totals with `Map.get_or_default`",
        "prompt": (
            "Write a single RSScript file that totals amounts per SKU. Write fn "
            "total_by_sku(skus: read List<String>, amounts: read List<Int>) -> fresh "
            "Map<String, Int> that walks the two parallel lists by index and accumulates "
            "each amount into the map, reading the running total with the core map "
            "get-or-default rather than branching on presence, and fn total_for(totals: read "
            "Map<String, Int>, sku: read String) -> Int returning 0 for an absent key. "
            "Mutation of the map is explicit at the call site. Add fn main() -> Unit that "
            "writes the total for a present key and for an absent one."
        ),
        "interfaces": [],
        "tags": ["collections", "map", "mut", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Map.get_or_default"},
            {"kind": "source_contains", "value": "map: mut"},
            {"kind": "diagnostic_absent", "value": "RS0202"},
        ],
    },
    {
        "id": "valve-state-machine",
        "title": "Drive a four-state valve machine held as a sum type",
        "prompt": (
            "Write a single RSScript file modelling a valve as a state machine. Declare sum "
            "ValveState with variants Closed, Opening(percent: Int), Open and Faulted(code: "
            "Int). Write fn step(state: read ValveState, command: read String) -> fresh "
            'ValveState where Closed moves to Opening(percent: 0) on "open", Opening '
            "advances by 50 until it reaches 100 and then becomes Open, Open returns to "
            'Closed on "close", and Faulted is unchanged; and fn label(state: read '
            "ValveState) -> fresh String rendering each state. Every match must cover all "
            "four variants, and an `if` must be a statement rather than a value. Add fn "
            "main() -> Unit that steps a closed valve four times with the open command and "
            "writes the final label."
        ),
        "interfaces": [],
        "tags": ["sum-type", "state-machine", "match", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "sum ValveState"},
            {"kind": "diagnostic_absent", "value": "RS0021"},
            {"kind": "diagnostic_absent", "value": "RS0209"},
        ],
    },
    {
        "id": "json-array-field-total",
        "title": "Parse JSON, walk an array and total one field",
        "prompt": (
            "Write a single RSScript file that totals one field across a JSON array. Write "
            "fn total_readings(text: read String) -> Result<Int, JsonError> that parses the "
            "text with the core Json interface, reads the object field `readings`, walks the "
            "array by index, reads the integer field `value` from each element, and returns "
            "the sum. Propagate every JsonError rather than unwrapping. Add fn main() -> "
            "Unit that calls it on a literal document with two readings and writes the total "
            "or the error message. Use only the core Json interface."
        ),
        "interfaces": [],
        "tags": ["json", "result", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Json.parse"},
            {"kind": "source_excludes", "value": ".unwrap()"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "option-chain-defaults",
        "title": "Chain optional lookups down to defaults",
        "prompt": (
            "Write a single RSScript file that reads a window definition out of one line of "
            "text, falling back to defaults. Declare struct Window with fields start: Int "
            "and width: Int. Write fn field_of(line: read String, delimiter: read String) -> "
            "Option<String> returning the text after the delimiter, fn parsed_field(line: "
            "read String, delimiter: read String) -> Option<Int> that chains that lookup "
            "into an integer parse with the core Option combinator, and fn window_from(line: "
            'read String) -> fresh Window that defaults start to 0 and width to 60, keeping '
            "a parsed width only when it is positive. Do not unwrap an Option. Add fn "
            "main() -> Unit that writes both resolved fields for a literal line."
        ),
        "interfaces": [],
        "tags": ["option", "defaults", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Option.unwrap_or"},
            {"kind": "source_excludes", "value": ".unwrap()"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "result-question-mapping",
        "title": "Propagate a domain error with `?` through three stages",
        "prompt": (
            "Write a single RSScript file that ingests amounts and reports a typed failure. "
            "Declare sum IngestError with variants Malformed(detail: String) and "
            "Rejected(detail: String), and fn IngestError.detail(error: read IngestError) -> "
            "fresh String rendering each. Write fn parse_amount(raw: read String) -> "
            "Result<Int, IngestError> returning Malformed when the trimmed text is not an "
            "integer, fn check_amount(amount: Int) -> Result<Int, IngestError> returning "
            "Rejected for a non-positive amount, and fn ingest(raw: read String) -> "
            "Result<Int, IngestError> that calls both with `?` and doubles the accepted "
            "amount. Write fn ingest_all(raws: read List<String>) -> Result<Int, String> "
            "that totals a list and returns the rendered detail of the first failure. Add fn "
            "main() -> Unit that writes the total or the error."
        ),
        "interfaces": [],
        "tags": ["result", "sum-type", "error-mapping", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "sum IngestError"},
            {"kind": "source_contains", "value": ")?"},
            {"kind": "diagnostic_absent", "value": "RS0021"},
        ],
    },
    {
        "id": "select-deadline-cancel",
        "title": "Race work against a deadline with `select` and cancel the loser",
        "prompt": (
            "Write a single RSScript file that races a unit of work against a deadline "
            "signal. Create two bounded Int channels of capacity 1 — one carrying the work "
            "result and one carrying the deadline marker — and take a sender and a receiver "
            "from each. Write one async fn per side that sends a value on its channel. "
            "Inside one task_group, start an async let for each, then use a `select` block "
            "with one arm per receiver, where each arm awaits the receive on that receiver; "
            "the deadline arm writes that the deadline fired and cancels an explicit "
            "CancellationSource. Await both async let handles in the same task_group. Note "
            "that `async let` handles cannot be awaited from inside a `select` arm: select "
            "arms await a direct async operation. Add fn main() -> Unit that runs it and "
            "reports the outcome."
        ),
        "interfaces": [],
        "tags": ["async", "task-group", "select", "cancellation", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "task_group"},
            {"kind": "source_contains", "value": "select {"},
            {"kind": "source_contains", "value": "CancellationSource.cancel"},
        ],
    },
    {
        "id": "two-host-ordered-calls",
        "title": "Call two declared host interfaces in a fixed order",
        "prompt": (
            "Write a single RSScript file that must use two separate declared host "
            "interfaces in order. The available host surface is exactly: "
            "Inventory.reserve(sku: read String, count: Int) -> Result<Int, String> and "
            "Inventory.release(sku: read String, count: Int) -> Unit, plus Sync.push(id: "
            "read String, amount: Int) -> Result<Int, SyncError>, SyncError.message(error: "
            "read SyncError) -> fresh String and Metrics.record(name: read String, value: "
            "Int) -> Unit. Write fn ship(sku: read String, count: Int) -> Result<Int, "
            "String> that reserves first and only then pushes the reserved amount; on a "
            "push failure it releases the reservation and returns the error message, and on "
            "success it records the accepted count with Metrics.record and returns it. Add "
            "fn main() -> Unit that ships one sku and writes the outcome. Do not invent, "
            "import, or assume any other host API."
        ),
        "interfaces": [INVENTORY, AUTOMATION],
        "tags": ["host-boundary", "external-symbol", "ordering", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "Inventory.reserve"},
            {"kind": "source_contains", "value": "Sync.push"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "list-index-assign",
        "title": "Clamp a list in place with index assignment",
        "prompt": (
            "Write a single RSScript file that clamps samples in place. Write fn "
            "clamp_samples(samples: read List<Int>, ceiling: Int) -> fresh List<Int> that "
            "copies the samples into a new mutable list and then walks it by index, "
            "replacing every element above the ceiling by assigning to that index directly "
            "rather than by rebuilding the list. Only a `let mut` binding can be assigned "
            "into. Add fn main() -> Unit that clamps a literal list, renders the elements as "
            "strings and writes them joined by commas."
        ),
        "interfaces": [],
        "tags": ["collections", "assignment", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "] = "},
            {"kind": "diagnostic_absent", "value": "RS0311"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "let-else-early-return",
        "title": "Return early from a failed pattern with `let ... else`",
        "prompt": (
            "Write a single RSScript file that parses a route and leaves early when it does "
            "not match. Declare struct Route with fields host: String and port: Int. Write "
            "fn parse_route(raw: read String) -> Option<Route> that splits the text on a "
            "colon, requires exactly two fields, trims both, and uses a `let ... else` "
            "binding for the parsed port so a non-integer port returns None without "
            "nesting; an empty host also returns None. Write fn describe_route(raw: read "
            'String) -> fresh String that uses `let ... else` on that Option and returns '
            '"unroutable" when it is None, otherwise `host/port`. The `else` block must '
            "diverge. Add fn main() -> Unit that writes both a routable and an unroutable "
            "example."
        ),
        "interfaces": [],
        "tags": ["option", "patterns", "control-flow", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "let Some("},
            {"kind": "source_contains", "value": "else {"},
            {"kind": "diagnostic_absent", "value": "RS0206"},
        ],
    },
    {
        "id": "tuple-destructure-caller",
        "title": "Return a tuple and destructure it at the call site",
        "prompt": (
            "Write a single RSScript file whose helper returns several counts at once. Write "
            "fn split_levels(lines: read List<String>) -> (Int, Int, Int) that counts the "
            'lines starting with "error", the lines starting with "warn", and everything '
            "else, and returns the three counts as one tuple. Add fn main() -> Unit that "
            "calls it on a literal three-line list, destructures the returned tuple into "
            "three bindings in a single `let`, and writes each count. Use only the core "
            "interfaces."
        ),
        "interfaces": [],
        "tags": ["tuples", "destructuring", "generation"],
        "invariants": [
            {"kind": "source_contains", "value": "-> (Int, Int, Int)"},
            {"kind": "source_contains", "value": "let ("},
            {"kind": "diagnostic_absent", "value": "RS0208"},
        ],
    },
]


def main() -> None:
    for task in TASKS:
        task_id = task["id"]
        toml_lines = [
            'schema = "rsscript.eval.task.v1"',
            f'id = "{task_id}"',
            "version = 1",
            'mode = "review"',
            f'title = "{task["title"]}"',
            f"prompt = {json.dumps(task['prompt'])}",
            f'candidate = "fixtures/{task_id}/candidate.rss"',
            "interfaces = ["
            + ", ".join(json.dumps(path) for path in task["interfaces"])
            + "]",
            f'expected = "expected/{task_id}.json"',
            "tags = [" + ", ".join(json.dumps(tag) for tag in task["tags"]) + "]",
        ]
        (EVALS / "tasks" / f"{task_id}.toml").write_text(
            "\n".join(toml_lines) + "\n", encoding="utf-8"
        )
        expected = {
            "task_id": task_id,
            "candidate_check": {"outcome": "pass", "diagnostic_codes": []},
            "target_check": {"outcome": "pass", "diagnostic_codes": []},
            "invariants": task["invariants"],
        }
        (EVALS / "expected" / f"{task_id}.json").write_text(
            json.dumps(expected, indent=2) + "\n", encoding="utf-8"
        )
    print(f"wrote {len(TASKS)} task/expected pairs")


if __name__ == "__main__":
    main()
