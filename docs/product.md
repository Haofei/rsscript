# Product direction

RSScript is a constrained scripting platform for embedded automation and
reviewable generated workflows. Its stable value is explicit program meaning:
ownership, retention, resource lifetime, structured concurrency, and external
calls whose semantic signatures can be inspected before execution.

## Users

The Core product serves Rust applications and services that embed scripts and
need deterministic compilation, replaceable host providers, bounded execution,
and machine-readable semantic analysis. It is not intended to compete with
general-purpose application languages or to act as a policy language.

Generated scripts are treated like any other input. Validation establishes
language correctness; it does not establish that arbitrary code is safe to run
inside the host process. Untrusted scripts require an independently isolated
runner as described in [threat-model.md](threat-model.md).

## Core workflow

```text
source + interfaces
  -> validation and neutral package analysis
  -> provider-independent Artifact Bundle
  -> optional policy-neutral semantic diff
  -> explicit Provider linking and signature validation
  -> bounded VM execution in-process or through the reference runner
  -> execution report
```

Successful reports identify the artifact, use a structured termination reason,
and record steps, cumulative allocation bytes, current/peak reachable VM value
storage, output bytes, intrinsic calls,
and Provider calls, including resources created, successfully cleaned, and
failed during cleanup. Low-overhead runtime telemetry adds execution and
cancellation latency, structured-task/resource peaks, and per-Provider-symbol
call, failure, logical payload-byte, total-duration, and maximum-duration
summaries. The logical payload estimate deliberately excludes allocator
capacity and Provider-specific transport framing. Reports serialize as the versioned
`rsscript.execution_report.v2` schema. Its mutually-exclusive `outcome`
contains either a canonical typed `WireValue` result or a machine-readable
failure; the historical v1 `NativeValue` projection is not emitted by the
reviewed SDK. Failed executions return a machine-readable termination reason
plus a diagnostic message instead of a bare string error.

The bytecode VM interpreter is the reference execution model. Rust AOT, native
plugins, REIR, and self-hosting remain optional Experimental, Integration, or
Research surfaces. The Cranelift tier is an explicit trusted-host performance
feature of the VM: it consumes the same verified bytecode, cannot be selected by
source or Artifact, and does not change language validity. Bounded and isolated
execution continue to use the interpreter until native execution provides the
same deterministic accounting.

Language, Artifact, runtime ABI, and Provider compatibility are independent,
fail-closed contracts defined in [compatibility.md](compatibility.md).

## Product invariants

- Provider selection does not change language validity or the provider-neutral
  executable artifact.
- Review consumes semantic facts; it never authorizes parsing, checking, or
  lowering.
- Host services are explicit interfaces resolved by providers at load time.
- Execution limits are availability controls, not permissions or isolation.
- Analysis and executable content in a Bundle share one digest-bound provenance
  record; semantic diff reports facts without authorizing execution.
- New syntax is frozen until semantic IR, provider ABI, bytecode verification,
  diagnostics, and VM conformance are stable.
