# ADR 0226: Post-migration product contracts

- Status: Accepted
- Date: 2026-08-16

## Context

The compiler-purity migration is complete. Keeping migration adapters, two
bytecode directions, and experimental backends indefinitely in the normal
product path would make every future change pay compatibility and backend costs
that ordinary embedders do not need.

## Decisions

1. The Core product path is immutable capture, frontend validation, typed MIR,
   `rsscript.bytecode.v1`, verification, host admission, Provider linking, and
   bounded VM or isolated-runner execution.
2. `rsscript.bytecode.v2` is a bounded numeric verifier prototype. It has no VM
   execution path, writer, or public deployment promise until a follow-up ADR
   defines its complete cutover: emitter, verifier, VM decoder, SDK/CLI use,
   v1 read-only fixtures, and v1 writer removal.
3. The runner profile is a host-owned deployment object. It selects installed
   Providers, limits, admission, and isolation controls. It does not take part
   in parsing, type checking, lowering, or Artifact identity. Its non-secret
   identity and descriptor digest are execution evidence.
4. Artifact digests establish content integrity and provenance binding. Origin
   authentication remains an optional `ArtifactAdmissionPolicy` responsibility;
   no signing hierarchy or language-level authorization system is introduced.
5. AOT, JIT, REIR, native plugins, and self-hosting are external experimental,
   integration, or research consumers. They may depend on stable Core contracts
   but must not be selected by the supported SDK execution, CLI execution,
   default VM, or release-validation closures. Architecture tests inspect those
   resolved dependency trees so an optional lab edge cannot silently re-enter
   the product path.
6. Compatibility APIs are transitional. New code uses canonical SDK modules and
   `WireValue`; compatibility stays explicitly feature-gated while the removal
   corpus is migrated.

## Consequences

The active engineering goal is contraction: remove compatibility call paths and
experimental reverse dependencies before adding syntax, providers, bytecode
formats, or backends. The completed migration checklist remains historical
evidence; durable invariants belong in architecture tests and ADRs.
