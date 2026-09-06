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

## Generation-facing direction

The current product direction is an **Agent-authored generation oracle**: a
machine-facing query that lets a code-generating agent ask what the current
source, interfaces, and compiler contracts permit, then receive a canonical
context and structured diagnostics suitable for an iteration or an evaluation.
It is a correctness aid for generated RSScript, not an authority grant or an
alternate compiler. Syntax owns prefix facts, semantics owns and composes typed
continuations, and compiler callers may consume the same result, as recorded in
[ADR 0232](architecture/adr/0232-parser-owned-generation-oracle.md).

The first implemented slice provides four cooperating outputs:

- structured diagnostics with stable stage, code, span, severity, and
  machine-readable context;
- a bounded continuation response that records source size, Core policy,
  interface revision, and stage outcome, plus generated language/schema
  metadata, without ambient host state;
- evaluation fixtures that measure whether an agent can recover from feedback
  without treating an incomplete answer as acceptance; and
- a sound success boundary: it reports success only after the owned stages have
  actually established their respective facts.

The v1 query and offline corpus are Experimental: terminal coverage and
semantic candidate coverage may remain explicitly `partial`. Machine
generation remains untrusted input; the parser, semantic validator, Artifact
admission, and execution boundaries remain the authorities for their existing
decisions.

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
- Generation feedback is canonical and evidence-bound when available; it never
  substitutes for parsing, semantic validation, compilation, Artifact admission,
  or execution isolation.
- Product evolution does not require splitting the repository, deleting an
  existing backend, or reordering the Cargo workspace.
