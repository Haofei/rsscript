# ADR 0228: Derive verified executable facts before persisting typed facts

- Status: Accepted
- Date: 2026-08-23
- Implementation: Phase 1 complete; Phase 2 requires a separate format ADR

## Problem

The native JIT currently reconstructs storage types and call facts from decoded
register bytecode. That repeats bounded data-flow work in whole-function, OSR,
and continuation compilation, creates several inference sites that can drift,
and makes static RSScript facts look like runtime specialization decisions.

The compiler and AOT integration have richer `SemanticTypeFacts`, but the
runtime trust boundary is different. An isolated runner may receive only a
verified Artifact. If semantic facts are not serialized and bound to that
Artifact, the runner cannot recover nominal types, generic arguments,
ownership transitions, or exact source contracts from register bytecode. A JIT
must not regain those facts by depending on syntax, HIR, MIR, the semantic
database, or compiler implementation types.

The architecture therefore needs two explicitly different fact layers:

1. facts that can be proven from the current verified executable without a
   format change; and
2. richer facts that require a new, versioned, digest-bound Artifact section.

Conflating those layers would either overstate what bytecode verification has
proved or silently couple runtime execution to compiler-only state.

## Decision and non-goals

### Phase 1: evaluation-local verified executable facts

The native VM path may derive an immutable `VerifiedExecutableFacts` projection
from an already verified and decoded `RegUnit`. Each function has a
`VerifiedFunctionFacts` entry keyed by the verified executable digest and
stable function ordinal.

The first projection is deliberately storage-oriented. Its leaf type is
`VerifiedStorageType`, with only distinctions the current executable can prove,
such as scalar machine classes, handles, and `Unknown`. It may also contain
bounded control-flow, resolved bytecode call-target, register definition/use,
and conservative instruction-effect facts that are derivable from the
verified instruction stream.

`Unknown` is a valid proof result. It causes a native optimization to decline,
retain a representation specialization, or use the interpreter. It must never
be replaced by an optimistic default.

The exact Rust field layout is an implementation detail, but the following
ownership rules are contractual:

- construction happens only after Artifact compatibility, payload, register,
  control-flow, import, and instruction verification;
- the projection is lazily cached by the verified executable and shared read-only
  with evaluation-local native state, not stored in `RegFunction` or the decoded
  executable contract;
- the projection is bound to that exact verified executable instance and indexed
  by stable function ordinal, never by an unbound source name or pointer;
- builds without `native-jit` do not construct or retain this projection;
- whole-function, OSR, and continuation translators consume the same facts;
- `NativeTy` remains a machine storage class derived from
  `VerifiedStorageType`, not a second language type inference system;
- facts are optimization evidence only. They do not authorize a Provider,
  weaken verification, change language validity, or change interpreter
  behavior.

The phase-1 data flow is:

```text
untrusted Artifact
        |
        v
BytecodeVerifier -> VerifiedBytecode -> decoded verified RegUnit
                                             |
                                             v
                              VerifiedExecutableFacts
                                             |
                            +----------------+----------------+
                            v                v                v
                     whole-function        OSR         continuation
                            |                |                |
                            +---------- native JIT -----------+
                                             |
                                  decline/deopt to interpreter
```

This phase does not change `rsscript.bytecode.v1`, the Artifact schema,
language semantics, Provider ABI, runtime ABI, or SDK API.

### Phase 2: persisted typed executable facts

Optimizations that require nominal layout identity, generic arguments, exact
call signatures, or program-point ownership and effect facts require a future
`TypedExecutableFactsV1` section. That section is a versioned executable
contract, not a serialization of compiler-owned `SemanticTypeFacts`.

At minimum the future envelope must bind:

- its own schema name and version;
- the canonical executable digest;
- the bytecode ISA and relevant runtime ABI versions;
- the interface-catalog or import-signature digest;
- stable function, instruction, type, layout, signature, and external-symbol
  identities;
- a digest of the canonical typed-facts payload.

The compiler/lowering path may emit the section from checked HIR/MIR mappings,
but only an independent bounded runtime verifier may construct the value made
available to execution backends. A backend receives a private verified wrapper,
never the decoded wire DTO.

The future section may express facts unavailable in v1 register bytecode,
including:

- nominal struct and variant layouts;
- generic instance arguments;
- statically resolved call signatures and `read`/`mut`/`take` effects;
- value-definition ownership and conservative alias classes;
- materialization recipes whose operands are independently checked.

Ownership is program-point or value-definition information. A single mutable
`ownership[register]` label is insufficient because moves, borrows, branches,
and register reuse change state over time.

`SemanticTypeFacts` remains a compiler/session model. It cannot be made
available after isolated Artifact loading without serialization, identity
binding, and independent verification. The VM and JIT are permanently
forbidden from depending on `rsscript-syntax`, `rsscript-semantics`, HIR, MIR,
or compiler crates to bypass that requirement.

### Non-goals

This decision does not:

- add a new executable format or typed-facts writer in phase 1;
- promise that v1 bytecode can recover erased nominal or generic information;
- make JIT facts part of language validity, review policy, or host admission;
- introduce JIT generic monomorphization, a new full-language typed IR, virtual
  objects, LICM, unrolling, SIMD, or runtime speculation;
- change the interpreter's role as the semantic oracle;
- permit persisted facts to widen Provider authority or runtime limits.

Those optimizations require focused follow-up decisions after the fact boundary
and performance evidence exist.

## Limits and construction contract

Phase-1 construction is bounded by the already accepted bytecode limits:

- function facts are one-for-one with verified functions;
- register facts are one-for-one with a function's verified register count;
- instruction and call-site facts are one-for-one with verified instructions;
- CFG edges come only from verified instruction successors;
- all aggregate fact-cell and byte-size calculations use checked arithmetic and
  a separate implementation limit before allocation;
- monotone data-flow uses a work queue and a bounded lattice; it may not rely on
  an unbounded rescan or recursive graph traversal;
- construction is a deterministic, non-recursive linear pass under a strict cell
  budget. Execution cancellation and deadline controls continue to govern script
  execution; phase 1 does not reinterpret them as Artifact-preparation failures.

The implementation must expose an explicit aggregate work or cell limit even
when the product of existing function/register/instruction limits would fit in
address space. Reaching any limit returns a typed decline or verification
failure and leaves interpreter execution available; it must not produce partial
facts.

`TypedExecutableFactsV1` requires its own bounded decoder. Before a future
writer is enabled, its format ADR must select concrete limits for section bytes,
type/layout/signature counts, per-function values, CFG edges, ownership events,
materialization nodes, and total fact cells. Nested records use explicit depth
and node limits. Generic arguments and materialization graphs may not be decoded
through an unconstrained recursive deserializer.

## Compatibility and migration

Phase 1 is evaluation-local and has no persisted compatibility surface. Native
cache keys include the executable digest and all fact representations that can
affect generated code. A change to fact derivation invalidates native cache
entries; it does not invalidate the underlying Artifact.

Phase 2 requires a separate cutover ADR before implementation. The expected
migration is:

1. define canonical wire DTOs, limits, and an independent verifier;
2. add malformed, tampered, N-1 reader, and deterministic-encoding fixtures;
3. emit the typed section as optional optimization evidence while
   `rsscript.bytecode.v1` remains the only executable payload;
4. let older runtimes ignore an unknown optional section;
5. let runtimes that understand the section expose it only after digest and
   structural verification;
6. allow a host admission profile to require a particular facts schema only by
   explicit policy.

An unknown required section remains an Artifact verification error. If an
optional typed-facts section is malformed, unsupported, or digest-mismatched,
no typed facts are admitted and native optimization requiring them is disabled.
The interpreter may still execute the independently verified bytecode unless
host admission explicitly requires typed facts. A recognized raw section is
never partially trusted.

Rollback is therefore straightforward: stop emitting the optional section and
continue producing the same verified bytecode. No language program or Provider
contract changes meaning. Persisted native machine code is outside this
decision and may not survive a facts-schema or derivation-version change.

## Verifier and security impact

The word `Verified` means that facts were either derived from an already
verified executable or admitted by the dedicated typed-facts verifier. It does
not mean the JIT can trust unchecked compiler assertions.

Security invariants are:

- every referenced function, register, instruction, layout, signature, and
  external symbol is range-checked against the exact executable;
- every static call and field fact agrees with the executable instruction at
  that program point;
- fact construction cannot add a call edge, suppress a write/effect, or
  describe a narrower effect than the verifier can prove;
- digest mismatches and invalid facts never reach code generation;
- facts do not bypass step, allocation, memory, intrinsic, Provider-call,
  cancellation, deadline, resource, or cleanup accounting;
- a JIT disagreement, unsupported `Unknown`, or failed guard declines or deopts
  to the interpreter without committing speculative effects;
- no fact changes the trusted-in-process status of native execution.

The typed-facts decoder/verifier is an untrusted-input boundary and therefore
requires fuzzing, malformed-input corpora, allocation limits, cancellation, and
deadline coverage before a writer ships.

## Provider and backend impact

Providers and Provider authors see no phase-1 API change. Provider imports and
their canonical signatures remain owned by the existing bytecode/ABI contracts.
Typed call facts may reference those identities in phase 2 but cannot introduce
or authorize a Provider binding.

The interpreter remains independent of native facts. AOT may continue consuming
compiler-owned semantic facts because it runs in the compiler closure, but AOT
and JIT must agree through differential tests rather than by importing each
other's internal type databases. Other backends may consume a future verified
typed-facts wrapper without depending on frontend crates.

## Acceptance criteria

Phase 1 is complete only when:

- facts are constructed exclusively from a decoded verified `RegUnit`, cached
  once by that executable, and shared with evaluation-local native state;
- non-`native-jit` builds allocate no executable facts;
- same executable digest and derivation version produce deterministic facts;
- fact construction has checked aggregate limits and never publishes a partial
  projection;
- scalar register storage types seed whole-function, OSR, and continuation
  translation from the shared projection, and compiled known-call paths consume
  its verified call-site signatures;
- ordinary scalar and known-call tests guard that verified facts are the primary
  source; local use-site inference remains only as a conservative v1 completion
  path for `Unknown` values;
- `Unknown` produces a safe decline/fallback and has focused differential tests;
- runtime shape keys retain only genuinely dynamic representations, not known
  scalar language types;
- architecture checks forbid VM/JIT dependencies on syntax, semantics, HIR,
  MIR, or compiler crates;
- interpreter/native differential, deopt, cancellation, deadline, and bounded
  construction tests pass.

Phase 2 may ship only when:

- `TypedExecutableFactsV1` has a separate accepted format/cutover ADR;
- canonical encoding, independent bounded decoding, digest binding, N-1
  fixtures, tamper tests, and a structured fuzz target exist;
- isolated Artifact loading reconstructs the same verified nominal, generic,
  call, layout, and ownership facts without frontend dependencies;
- old readers continue to execute the canonical bytecode according to the
  declared optional-section policy;
- no backend can construct or mutate the verified typed-facts wrapper;
- removing the section demonstrably falls back to the verified interpreter
  without changing program semantics.

## Evidence

The phase-1 implementation must add unit tests for fact derivation and limits,
architecture tests for the dependency boundary, and native differential tests
covering known, unknown, and conflicting storage facts. The future phase-2
implementation additionally requires typed-section decoder fuzzing,
cross-platform deterministic fixtures, digest-tampering fixtures, old-reader
fixtures, and an isolated-runner round trip.
