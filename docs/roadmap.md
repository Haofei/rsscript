# RSScript Roadmap

This is the only project roadmap. It is non-normative: specifications and tests
win when status prose becomes stale. The current product scope is defined in
[support.md](support.md).

## Priority 0: Close Trust Boundaries

These items block stronger deployment claims:

1. Capture a complete immutable package and dependency snapshot before review,
   lowering, build, or publication.
2. Carry one mandatory execution policy through VM intrinsics, generated AOT
   programs, native loading, JIT, GPU, process, network, filesystem, and
   database capabilities.
3. Extend the completed Linux killable-worker boundary to audited Windows and
   macOS launchers without weakening bounded IPC, credential denial, filesystem
   policy, or process-tree termination.
4. Complete Windows secure-store SID/DACL validation and suspended
   create/Job-assign/resume process launch.
5. Replace path/string authority with scoped handles where host resources are
   exposed.

The bounded reference VM carries a mandatory context and supports pure
`trusted-ci` execution with no ambient host authority. AOT and host effects
remain fail-closed for that profile. Linux `untrusted-isolated` execution uses
the versioned worker protocol and verified bubblewrap launcher; unsupported
platforms and launchers fail closed.

## Priority 1: Strengthen The Core Product

Focus changes on the review-first path:

- make checked semantic facts the single source for review, lowering, VM, LSP,
  and package tooling;
- replace display-string type substitution with structural typed substitution;
- improve package and REIR evidence provenance and independent capability
  verification;
- make parser, analyzer, LSP, REIR, and adapters consume one operation budget
  with cancellation and deadlines;
- improve semantic diagnostics, fixes, and source maps before adding language
  surface;
- broaden cross-isolate payloads only when a real isolate model and ownership
  proof exist.

## Priority 2: Reduce Maintenance Cost

Decompose by invariant, not by line count:

1. Native loader/build/snapshot boundaries.
2. LSP document store, scheduler, analysis, publication, and features.
3. REIR model, indexing, reconciliation, I/O, and rendering.
4. Runtime process/network/database services and explicit injected context.
5. Analyzer and Rust lowering around a sealed validated semantic model.
6. VM/JIT validation, ABI, executable memory, deopt, OSR, and code generation.

Public compatibility aliases and broad glob exports should contract before a
stable API is declared.

## Experimental Work

The following work is frozen against scope expansion. Correctness, security,
measured regressions, and maintenance fixes are allowed; promotion requires an
explicit support-policy change.

### JIT

Keep Cranelift and the existing differential/parity contract. Near-term work is
limited to:

- compilation and executable-memory hard limits;
- precise deopt state and heap-aware region coverage;
- effect/alias-aware collection metadata caching;
- proven list range/store-load optimizations;
- helper-call fusion where measurements show a real win;
- deterministic counters and performance gates.

Do not add a second machine-code backend, SIMD, or broad speculation without a
measured workload that justifies the additional verification matrix.

### Metal And Native Plugins

Do not broaden dynamic shader or in-process plugin surfaces. The next meaningful
security feature is worker isolation, not more heuristic source classification.

### Self-Hosting

Self-hosting remains a parity and architecture pressure test, not a release
requirement. Its current contract and next milestones live in
[self-hosting.md](self-hosting.md).

### ML, Rewrite, And Cross-Isolate Research

Declarative rewrite scheduling, ML performance work, and true multi-isolate
execution remain research topics. They should not create Core APIs until a
measured product workload and a complete ownership/resource model exist.

## Promotion Rule

An Experimental feature moves to Core only when it has:

- a stable user-facing contract;
- default-off to default-on migration evidence where applicable;
- per-PR tests on supported platforms;
- bounded resource and failure behavior;
- a threat model consistent with its deployment profile;
- a rollback plan and benchmark or user evidence.

Completed task lists and measurements belong in Git history or benchmark data,
not in this roadmap.
