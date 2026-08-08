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
  -> provider-independent executable artifact
  -> explicit Provider linking and signature validation
  -> bounded VM execution
  -> execution report
```

Successful reports identify the artifact, use a structured termination reason,
and record steps, cumulative allocation bytes, output bytes, intrinsic calls,
and Provider calls, including resources created, successfully cleaned, and
failed during cleanup. Low-overhead runtime telemetry adds execution and
cancellation latency, structured-task/resource peaks, and per-Provider-symbol
call, failure, logical payload-byte, total-duration, and maximum-duration
summaries. The logical payload estimate deliberately excludes allocator
capacity and Provider-specific transport framing. Reports serialize as the versioned
`rsscript.execution_report.v1` schema. Failed executions return a
machine-readable termination reason plus a diagnostic message instead of a bare
string error.

The bytecode VM is the reference execution model. Rust AOT, Cranelift JIT,
native plugins, REIR, and self-hosting remain optional Experimental, Integration,
or Research surfaces. They must not create dependencies in the Core compiler or
change language validity.

Language, Artifact, runtime ABI, and Provider compatibility are independent,
fail-closed contracts defined in [compatibility.md](compatibility.md).

## Product invariants

- Provider selection does not change language validity or the provider-neutral
  executable artifact.
- Review consumes semantic facts; it never authorizes parsing, checking, or
  lowering.
- Host services are explicit interfaces resolved by providers at load time.
- Execution limits are availability controls, not permissions or isolation.
- New syntax is frozen until semantic IR, provider ABI, bytecode verification,
  diagnostics, and VM conformance are stable.
