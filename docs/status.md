# Current Project Status

This is the single current engineering-status document. It records boundaries,
not a chronological work log. Detailed remediation history remains available in
Git.

## Product State

RSScript is a `0.1.x` review-first language and evidence prototype. It is useful
for trusted local development, semantic package review, REIR generation, and
controlled CI experiments. It is not a sandbox, formal proof system, stable
registry, or production multi-tenant execution platform.

The binding support and deployment matrix is [support.md](support.md).

## Established Boundaries

- Compiler and review core forbid unsafe Rust; unsafe code is isolated in
  dedicated JIT, ABI, process, and GPU crates.
- Unknown review evidence remains explicit and production REIR policy is
  fail-closed.
- Package inputs and artifacts use bounded traversal, content identities,
  no-follow checks, staged writes, and atomic publication where supported.
- Native builds preserve reviewed features, run offline/frozen, and load
  digest-verified artifacts from private storage.
- VM and process paths have default time, output, memory/work, host-call, and
  process-tree controls. Unlimited/native modes require explicit trusted flags.
- Runtime network, HTTP, filesystem, database, channel, and stream paths have
  bounded variants and typed errors.
- Native ABI buffers are shape-checked before ownership transfer and released
  through RAII on valid success paths.
- LSP uses immutable snapshots, revisions, cancellation, bounded scheduling,
  and publication outside global state locks.
- REIR reconciliation has indexed exact matching and an operation budget.
- CI pins actions/toolchains, audits dependencies, separates Core and
  Experimental coverage, and promotes the same validated release artifact.

These controls reduce mistakes and denial-of-service exposure. They do not turn
host execution into isolation.

## Open Security And Correctness Work

| Area | Current limitation | Required closure |
| --- | --- | --- |
| Package authorization | Review can precede capture of the complete dependency closure | Snapshot first; review, lower, build, and publish only that immutable graph |
| Deployment profile | CLI fails closed outside `local-trusted`, but embedding APIs do not carry one mandatory policy | End-to-end execution context and capability checks |
| Native/JIT/GPU | Trusted code still runs in the host process | Killable OS-isolated workers with bounded IPC |
| Windows | Secure cache ACL and atomic Job attachment remain incomplete | SID/DACL validation and suspended process launch |
| Host authority | Some APIs still accept paths, URLs, commands, and DSNs as authority | Root/endpoint/executable/database capability handles |
| Capability evidence | Some native capability facts are author declarations | Independent verification and provenance |
| Frontend budgets | Limits exist in several phases but are not one end-to-end contract | Unified source/token/depth/node/diagnostic budget |
| External integrations | Live PostgreSQL and broader hardware coverage are environment-gated | Dedicated, auditable integration runners |

## Open Maintainability Work

- Introduce a sealed semantic database/validated-program boundary.
- Replace string-based generic type substitution.
- Continue migrating runtime APIs to `OperationContext`, then an injected
  execution context.
- Split LSP, REIR, analyzer, lowering, runtime services, and VM/JIT by
  invariant.
- Reduce broad public re-exports before declaring API stability.
- Replace remaining global registries with explicit owner/session lifetimes.

These are not reported as correctness fixes until executable invariants and
regression tests exist.

## Experimental Status

- Native JIT and Metal have dedicated path-triggered, nightly, and release
  validation. They are not Core.
- Self-hosting proves substantial lexer/parser/checker parity but is not an
  independent compiler or release requirement.
- Package publish remains a dry-run validation surface, not a hosted registry.
- True multi-isolate execution, general ML scheduling, and declarative rewrite
  systems are research, not committed product surface.

## Documentation Policy

This file replaces dated remediation ledgers and checked-in review reports.
When an item closes, update the relevant row and its tests in the same change.
Do not add a new dated status file.
