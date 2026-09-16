# ADR 0235: Native engine telemetry durations are `u64` nanoseconds

- Status: Accepted
- Date: 2026-09-15

## Problem

`ExecutionEngineTelemetryV2::Native` — the seven-counter native-engine summary
in the runner protocol's execution report — declared its two duration counters
as `u128`:

```rust
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionEngineTelemetryV2 {
    Interpreter,
    Native { /* … */ compile_nanos: u128, run_nanos: u128 },
}
```

An internally tagged enum cannot deserialize that. Serde has to read the `kind`
tag before it knows which variant's fields to expect, so it buffers the object
into `serde_private::de::Content`, and `Content` has no 128-bit carrier: a
`u128` in a buffered position fails with `i128 is not supported`. The variant
could be constructed and serialized, and then nothing could read it back. Every
other execution-report field round-tripped; this one silently did not.

The CLI is where that showed. `rss run --trusted-in-process --native --json`
prints a report typed as `ExecutionReportV2`, and `crates/rsscript-cli/src/cli/runner.rs`
could not simply reparse the in-process report into that type — it projected the
engine telemetry onto the parsed report by hand, out of a `serde_json::Value`,
partly to narrow the VM's full diagnostic counter set to the protocol's summary
and partly because the summary itself was unparseable. The second half of that
workaround was covering a defect in the contract, and it meant no consumer
outside this repository could parse a native report at all.

## Decision and non-goals

Both counters become `u64` nanoseconds:

```rust
Native { /* … */ compile_nanos: u64, run_nanos: u64 },
```

`u64` nanoseconds span 584 years, which is not a bound any process reaches, so
nothing representable is lost. The conversion happens at the producer, not at
the boundary: `rsscript-vm`'s `NativeExecutionEngineTelemetry` — the report type
the SDK re-exports — now carries these two fields as `u64` and narrows the VM's
internal `u128` accumulators with a saturating `u64::try_from(…).unwrap_or(u64::MAX)`
in `reg_vm/executable.rs`. Saturation is the honest projection: a run past the
ceiling reports the ceiling rather than wrapping to a small number.

Non-goals. The VM's internal JIT accumulators stay `u128`; they are summed
per-compile and never cross a wire. The four phase counters on the report type
(`translation_nanos`, `validation_nanos`, `codegen_nanos`, `finalize_nanos`)
stay `u128` because they are diagnostic fields that the runner protocol's
summary does not republish — only the two fields the protocol carries are
narrowed, and their doc comments say why. The CLI's projection function is not
removed: the VM's native report still carries the full diagnostic counter set
while the protocol variant denies unknown fields, so the narrowing step remains
for that reason alone. What changed is that its output is now parseable, which
is what the new CLI test asserts.

## Compatibility and migration

There is no migration, and there is nothing to migrate.

On the wire the JSON is unchanged for every value below 2^64. Both widths
serialize a nanosecond count as a bare JSON integer; `serde_json` writes
`"compile_nanos": 123456789` either way. A reader that accepted the old
serialized shape accepts the new one byte-for-byte.

And no consumer could have parsed the old shape: deserializing
`ExecutionEngineTelemetryV2::Native` failed unconditionally, for every value,
because the failure was in the buffering of the tagged enum and not in any
particular number. So there is no stored report, no downstream decoder, and no
released consumer whose behaviour this can change — only decoders that used to
fail and now succeed.

`schemas/rsscript.execution_report.v2.schema.json` is unchanged: it types
`telemetry` as `{"type": "object"}` and never constrained the engine summary's
numeric width, so the checked-in schema described the new shape already.

The isolated runner path is unaffected in either direction: its child never
selects the native tier, so it only ever emitted the payload-free `interpreter`
variant.

## Verifier and security impact

None. Execution telemetry is report output, not verifier input: no verification
decision, resource limit, or trust boundary reads these counters. The change
narrows a serialized integer's declared range, which can only make a decoder
stricter, never a program more privileged. Saturating at the producer means a
pathological duration reports `u64::MAX` rather than wrapping into a plausible
small value that could understate cost.

## Provider and backend impact

No Provider ABI, bytecode, MIR, or Artifact change. Inside the VM, the native
tier's accumulation is untouched; only the projection into the report narrows.
`rsscript-sdk`'s `native_jit_scorecard` reads `run_nanos` as `u64` for its
per-entry division, which is the only arithmetic in the tree that mixed widths.

## Evidence

- `crates/rsscript-runner-protocol/src/lib.rs`:
  `native_engine_telemetry_round_trips_through_json` serializes a `Native`
  summary (including `run_nanos: u64::MAX`) and deserializes it back, then does
  the same through a whole `ExecutionReportV2`. Under the old `u128` fields this
  test cannot pass.
- `crates/rsscript-cli/tests/cli.rs`:
  `the_native_json_report_parses_as_the_typed_v2_contract` runs
  `rss run --trusted-in-process --native --json` and requires
  `serde_json::from_str::<ExecutionReportV2>` to accept exactly the bytes the
  CLI printed, with native engine telemetry present.
