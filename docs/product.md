# Product direction

RSScript is a constrained scripting platform for embedded automation and
reviewable generated workflows. Its stable value is explicit program meaning:
ownership, retention, resource lifetime, structured concurrency, and external
calls whose semantic signatures can be inspected before anything executes.

## Users

The Core product serves Rust applications and services that embed scripts and
need four things: deterministic compilation, replaceable host providers,
bounded execution, and machine-readable semantic analysis. That is the whole
target. General-purpose application languages and policy languages solve
different problems.

Generated scripts are ordinary input to this pipeline. Validation establishes
that a program means what it says; deciding that a program is safe to run inside
the host process is the host's call, and untrusted scripts belong in the
independently isolated runner described in [threat-model.md](threat-model.md).

## Generation-facing direction

The current product direction is an **Agent-authored generation oracle**: a
machine-facing query that lets a code-generating agent ask what the current
source, interfaces, and compiler contracts permit, then receive a canonical
context and structured diagnostics suitable for an iteration or an evaluation.
It is a correctness aid for generated RSScript. Syntax owns prefix facts,
semantics owns and composes typed continuations, and compiler callers consume
that same result rather than a second oracle, as recorded in
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

The v1 query and offline corpus are Experimental: terminal coverage and semantic
candidate coverage may remain explicitly `partial`. Machine generation is
untrusted input like any other, and the parser, semantic validator, Artifact
admission, and execution boundaries stay the authorities for their own
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
storage, output bytes, intrinsic calls, and Provider calls, including resources
created, successfully cleaned, and failed during cleanup. Low-overhead runtime
telemetry adds execution and cancellation latency, structured-task/resource
peaks, and per-Provider-symbol call, failure, logical payload-byte,
total-duration, and maximum-duration summaries. The logical payload estimate
deliberately excludes allocator capacity and Provider-specific transport
framing. Reports serialize as the versioned `rsscript.execution_report.v2`
schema, whose mutually-exclusive `outcome` carries either a canonical typed
`WireValue` result or a machine-readable failure; the reviewed SDK emits that
schema only. A failed execution returns a machine-readable termination reason
and a diagnostic message.

## Execution tiers

The bytecode VM interpreter is the reference execution model and the semantic
oracle for every other tier. The Cranelift JIT is an explicit trusted-host
performance feature of the VM and a required part of the product for CPU-bound
embedded workloads: it consumes the same verified bytecode, is selected by the
host rather than by source or by an Artifact, and leaves language validity
untouched. Native regions report the interpreter's exact step and
intrinsic-call counts and stop for the same budget, cancellation, and deadline
reasons, so in-process execution can select the JIT under the standard runner
limit profile. The isolated runner stays on the interpreter until allocation
controls admit call-bearing regions and the runner profile exposes native
selection; closing that gap is roadmap work, as recorded in
[ADR 0233](architecture/adr/0233-jit-is-a-product-owned-vm-tier.md). Native
plugins remain an optional Experimental surface. Rust AOT, REIR, and
self-hosting are archived on the `archive/experiments-2026-09` branch.

Language, Artifact, runtime ABI, and Provider compatibility are independent,
fail-closed contracts defined in [compatibility.md](compatibility.md).

## Product invariants

- Provider selection changes neither language validity nor the provider-neutral
  executable artifact.
- Review consumes semantic facts; parsing, checking, and lowering answer only to
  the language.
- Host services are explicit interfaces resolved by providers at load time.
- Execution limits are availability controls, not permissions or isolation.
- Analysis and executable content in a Bundle share one digest-bound provenance
  record; semantic diff reports facts and leaves execution decisions to the
  host.
- New syntax is frozen until semantic IR, provider ABI, bytecode verification,
  diagnostics, and VM conformance are stable.
- Generation feedback is canonical and evidence-bound when available, and sits
  alongside parsing, semantic validation, compilation, Artifact admission, and
  execution isolation rather than in place of any of them.
- Product evolution keeps the current repository and workspace shape: one
  repository, the existing backends, and the current Cargo workspace order.

## Maturity

`Core` means the contract is part of the supported product. `Experimental`
means correctness is tested but the API or backend may change. `Integration`
means an optional consumer of Core artifacts. `Research` is retained for
regression value and does not drive the product roadmap.

| Surface | Maturity | Default | Contract |
| --- | --- | --- | --- |
| Parser, formatter, baseline diagnostics | Core | On | Language specification |
| Agent-authored generation oracle | Experimental | On in CLI | `rsscript.generate.*.v1`; [ADR 0232](architecture/adr/0232-parser-owned-generation-oracle.md) |
| Structured diagnostics and machine fixes | Core | On | `rss check --json` and `rss fix --json` |
| Generated machine language context | Experimental | On | `docs/generated/*.json` with freshness checks |
| Generation-oracle evaluation corpus | Experimental | Explicit tooling | `rsscript.agent_eval.v1`; deterministic offline fixtures |
| Types, ownership, retention, resources | Core | On | Language specification |
| Structured async and cancellation semantics | Core | On | Language and execution specifications |
| External semantic symbols | Core | On | Interface and binding schemas |
| Register VM | Core | On | Reference execution semantics |
| Package snapshot and neutral analysis | Core | On | Versioned artifact schemas |
| Artifact Bundle and semantic diff | Core | On | Bundle and `rsscript.semantic_diff.v2` schemas |
| Reference isolated runner | Experimental | On in CLI | Runner protocol v1 and process limits |
| Host providers | Experimental | Explicit | Provider ABI and runner configuration |
| Cranelift JIT, OSR, deopt | Experimental (active) | Explicit trusted host | Differential parity, safe fallback, and workload evidence; see [roadmap.md](roadmap.md) |
| Native plugins | Experimental | Off | Trusted provider boundary only |
| Rust AOT, REIR review, self-host frontend | Archived | Off | `archive/experiments-2026-09` branch; not workspace members |

A feature is not promoted by implementation count. Promotion requires a stable
contract, conformance coverage, bounded failure behavior, supported-platform CI,
and a threat model consistent with [threat-model.md](threat-model.md).

Rust AOT, REIR, and self-hosting are archived and receive no changes. The
Cranelift JIT is different: it is a required VM tier under
active development, and its Experimental maturity records that its accounting
and platform contracts are still converging, not that it is a removal
candidate. The generation-oracle work does not split this repository, delete a
backend, or reorder the Cargo workspace.

## Language conformance

`✓` is Core coverage; `Experimental` and `Partial` are not Core support claims.

| Language area | Spec | Parser | Semantics | VM | JIT | LSP | Tests |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Ownership effects | ✓ | ✓ | ✓ | ✓ | Experimental | ✓ | ✓ |
| Retention / escape | ✓ | ✓ | ✓ | ✓ | Experimental | ✓ | ✓ |
| Resource lifetime | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |
| Structured async | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |
| Cancellation cleanup | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |
| External symbols | ✓ | ✓ | ✓ | ✓ | Experimental | ✓ | ✓ |
| Dynamic protocols | ✓ | ✓ | ✓ | ✓ | Partial | ✓ | ✓ |

Parser acceptance alone never marks a language area supported. Core promotion
requires semantic validation, verified-VM conformance, diagnostics, LSP behavior,
and regression coverage together.

## Support boundary

Execution limits, cancellation, deadlines, output caps, and child-process
limits are supported availability controls; none of them, and neither the
in-process VM, the JIT, nor a Provider, is a security sandbox. Untrusted,
third-party, or machine-generated scripts require the isolated runner;
successful validation is not authorization to execute in the host process.

Official host Providers remain Experimental and fail closed where their stated
authority boundary cannot be implemented: the rooted filesystem Provider
requires the descriptor-relative, no-follow Unix implementation and rejects
construction elsewhere rather than falling back to canonicalize-then-open. The
reference runner remains Experimental until its platform isolation matrix is
complete. Rust AOT, REIR, and self-hosting are archived on the
`archive/experiments-2026-09` branch and are not supported surfaces.
