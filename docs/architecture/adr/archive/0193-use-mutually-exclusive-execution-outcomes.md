# ADR 0193: Use mutually exclusive execution outcomes

- Status: Accepted
- Date: 2026-08-14

## Problem

The reviewed Rust `ExecutionReport` exposed independent `value`,
`display_value`, `termination_reason`, and optional `failure` fields. Callers
could observe or construct combinations such as a completed reason plus a
failure, or a failed run with a return value. That weakened the phase boundary
the SDK is intended to provide.

## Decision

The reviewed report owns one `ExecutionOutcome`:

* `Completed { wire_value, display_value }`; or
* `Failed(RuntimeError)`.

The report derives its termination reason and optional success/failure accessors
from that enum. The reviewed JSON contract is
`rsscript.execution_report.v2`, which serializes the same mutually exclusive
outcome. The historical v1 projection is retained only as a read-only
compatibility fixture; runner and machine consumers use v2.

## Consequences

Script, Provider, budget, cancellation, and deadline terminal paths retain a
complete report with exactly one terminal state. Verification, linking, and
host/protocol failures remain outer errors because no execution has occurred.
