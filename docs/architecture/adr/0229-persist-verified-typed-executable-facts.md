# ADR 0229: Persist optional executable-bound typed facts

- Status: Accepted
- Date: 2026-08-23
- Supersedes: the Phase 2 deferral in ADR 0228

## Context

ADR 0228 introduced evaluation-local facts that the VM can conservatively
derive from verified register bytecode. That projection intentionally cannot
recover nominal layouts, generic substitutions, exact call effects, or source
ownership facts erased by bytecode v1. Reconstructing those facts in the JIT
would create a second type checker and would make generated native code depend
on guesses.

The compiler already owns the required mapping while lowering verified MIR to
register bytecode. Isolated execution, however, may receive only an Artifact,
so compiler-owned memory cannot be trusted or consulted at execution time.

## Decision

Bytecode v1 may contain an optional `rsscript.typed_executable_facts.v1`
section. The executable payload remains the only persisted executable contract;
typed facts are optimization evidence and never change interpreter semantics,
Provider authority, language validity, or admission policy.

The data flow is:

```text
verified MIR
  -> bytecode lowering + stable identity mapping
  -> executable payload + optional typed-facts section
  -> independent bounded BytecodeVerifier
  -> VerifiedBytecode { optional BoundTypedExecutableFactsV1 }
  -> VM projection
  -> whole-function / OSR / continuation JIT
```

Only the bytecode verifier can construct `BoundTypedExecutableFactsV1`. The
name is intentional: it proves canonical structure, resource bounds, and
binding to this verified executable, not a second full language type/effect
proof. The VM consumes that wrapper directly, intersects it with its
independently derived executable facts, and declines or rejects conflicts; it
does not decode or re-verify raw facts. Artifacts without the optional section
remain valid and use the conservative ADR 0228 derivation.

The section is bound to:

- the canonical executable digest;
- bytecode ISA and runtime ABI versions;
- the interface-catalog digest and canonical import-table digest of the
  enclosing Artifact;
- stable function and instruction ordinals;
- stable nominal layout identities assigned after deterministic sorting.

Known sections 1-4 remain required. The typed-facts section is optional so an
older reader can skip it. A runtime that recognizes the section rejects a
malformed, non-canonical, mismatched, or over-budget section rather than
partially trusting or silently downgrading it. Removing the section restores
the old-artifact fallback path.

## Bounded verification

Verification occurs before an execution backend receives facts and enforces:

- an independent section byte limit;
- exact function and register cardinality agreement with the executable;
- bounded call sites, layouts, fields, generic arguments, and nested type
  depth;
- canonical CBOR and stable ordering;
- call-site opcode, target and argument arity, plus exact Provider import
  parameter/result/effect contracts;
- bounded register, instruction, function, import, and layout identities;
- checked arithmetic for aggregate counts;
- cancellation and deadline checks during verification.

Unknown or unavailable facts are represented explicitly. They authorize no
optimization. In particular, lowering does not infer generic substitutions
from source spellings after the semantic boundary.

The first verifier proves the envelope binding, canonical structure, bounds,
call-target identity, and typed-intrinsic argument carried by bytecode v1. It
does not yet repeat a complete language type-dataflow proof. Therefore the VM
only accepts a persisted scalar storage class when it agrees with the class
independently derived from executable bytecode; a bytecode-local `Unknown` is
not promoted by the section. Conflicts fail closed before native compilation.
Ordinary function generic substitutions must remain empty because v1
`CallKnown` carries no substitution proof. Only the explicit type argument of
`CallTypedIntrinsic` may be persisted, and it must exactly match that
instruction.

Register ownership in this first schema is conservative boundary evidence.
It is not a program-point borrow state or a complete alias proof and therefore
cannot alone authorize store elimination or mutable-alias optimizations.

## JIT contract

`VerifiedJitType` is a language/storage fact. `NativeTy` remains only the
machine representation selected from a verified fact. Runtime shape keys may
contain genuinely dynamic representation information such as flat versus boxed
lists or a dynamic closure target; statically known scalar and nominal types do
not create shape versions.

Evaluation-local typed regions may derive conservative ownership, alias,
field, escape, and materialization facts. A conflict or `Unknown` declines the
optimization. Virtual-object analysis is a shared framework for existing
Option/Result/Variant reconstruction and future aggregates; it does not by
itself enable allocation elimination, struct scalar replacement, speculation,
native recursion, SIMD, or another JIT tier.

## Compatibility and rollback

- Existing bytecode v1 artifacts without typed facts remain readable.
- The executable checksum and execution result are unchanged by omitting the
  optional evidence.
- Deterministic build tests cover section ordering and byte-for-byte output.
- Malformed and digest-transplant fixtures are permanent compatibility corpus.
- Native caches are evaluation-local and include every static fact and dynamic
  representation dimension that can affect generated code.
- A writer rollback consists of stopping section emission; no source or
  Provider contract migration is required.

## Performance governance

Typed facts are infrastructure, not proof that an optimization is profitable.
Static inlining, virtual objects, loop canonicalization, range analysis, LICM,
unrolling, and selective runtime specialization remain subject to differential
correctness, bounded compilation work, code-size limits, and canonical workload
scorecards. SIMD and new speculation are not enabled without a separate ADR and
retention evidence.
