# RSScript roadmap

This roadmap implements the product boundary in [product.md](product.md). The
language specification and tests remain authoritative for existing behavior.

## Completed product-boundary milestones

1. Removed active policy/capability examples and obsolete product descriptions.
2. Replaced catalog-size assertions with uniqueness, completeness, signature,
   determinism, and orphan-entry checks.
3. Split neutral package analysis from review, provider, native, and build
   metadata, with checked-in schemas.
4. Enforced dependency direction using Cargo metadata.
5. Established syntax, structural semantics, Typed HIR, executable IR, provider
   ABI, host-neutral runtime defaults, and concrete leaf providers.
6. Added bounded `rsscript.bytecode.v1`, structural verification, verified-only
   VM construction, a stable embedding façade, build/inspect commands, and the
   provider-replacement demonstration.
7. Defined the strict execution-report contract, added a reusable Provider
   Conformance Kit for all official Providers, and added property/fuzz coverage
   for ownership, retention, resource handles, Artifact bytes, bindings, and
   report consumers.
8. Established a versioned Core SLO/reporting gate and three-platform CI
   evidence with checksums and provenance.
9. Split the reference VM into `rsscript-vm`, established the owned CFG
   `rsscript-mir` model and `rsscript-codegen-vm` boundary, and made checked-HIR
   lowering a one-way compiler projection. Cargo metadata tests now reject any
   VM dependency on compiler, syntax, semantics, or lowering internals.

## Current priority: generation oracle and machine feedback

The compiler-purity migration is complete. Its checked baseline is retained as
historical evidence, not as the active roadmap. The active work is to define a
sound, Agent-authored generation oracle around the existing Core path. It must
make feedback usable by machines without creating a parallel parser, checker,
compiler, or trust boundary.

1. Maintain the parser-owned v1 generation query: syntax supplies prefix and
   terminal facts, semantics supplies and composes scoped/type/effect facts, and
   compiler callers reuse that contract without owning a second oracle. See
   [ADR 0232](architecture/adr/0232-parser-owned-generation-oracle.md).
2. Expand parser terminal coverage without weakening the explicit completeness
   flags or the rule that `may_stop` requires complete syntax and valid
   semantics. Keep `rss check --json` and `rss fix --json` as the diagnostic and
   repair contracts.
3. Strengthen the generated machine context and interface identity beyond the
   current versioned language card, grammar hash, Core interface sources, Core
   policy, and per-session interface revision. It excludes host secrets,
   deployment policy, and Provider implementation state.
4. Grow the offline evaluation corpus beyond its initial fixtures and populate
   all four generation modes with caller-supplied model samples. Evaluations may
   demonstrate progress, but no incomplete query result is a successful compile
   or an execution authorization.
5. Keep the archived research archived. Rust AOT, REIR, and self-hosting
   live on the `archive/experiments-2026-09` branch; they are not workspace
   members and do not return as SDK, VM, or CLI dependencies.
6. Preserve the current repository and workspace shape: do not split the
   repository, delete backends, or reorder the Cargo workspace as part of this
   work. Keep `rsscript.bytecode.v1`, host deployment boundaries, and the
   capture/build/verify/inspect/default-isolated-run golden path intact.

## Parallel priority: native JIT as a supported VM tier

The Cranelift JIT is a required part of the product, not an experiment awaiting
a retention verdict. CPU-bound embedded workloads need native execution, and
the interpreter alone does not meet that need. The JIT remains an opt-in VM
feature that consumes the same verified bytecode and cannot be selected by
source or Artifact, and the interpreter remains the semantic oracle for every
native path. Within those invariants, JIT work is active feature work:

1. Close the accounting gap: native execution must report the same
   deterministic step, allocation, cancellation, and deadline facts as the
   interpreter, so bounded and isolated execution can use it.
2. Grow native coverage of the language so fewer regions fall back to the
   interpreter, with differential parity gates for every new region kind.
3. Keep the controlled workload scorecard as the evidence for each
   optimization; measured regressions are removed, but the engine itself is
   not a removal candidate.
4. Promote the engine to Core once accounting parity, supported-platform CI,
   and the threat model align, following [feature-matrix.md](feature-matrix.md).

## Frozen scope

Until the priorities above are complete, do not expand language syntax, public
intrinsics, speculative JIT tiers without scorecard evidence, the C backend,
full self-host bootstrap, package publishing, native plugin surface, or
language-level policy. Correctness, security boundary, maintenance, and
measured-regression fixes remain allowed. Syntax sugar that desugars in the
parser to an existing AST node and is justified by a measured generation failure
is also permitted; new semantics are not.

Rust AOT, REIR, and self-hosting are archived on the
`archive/experiments-2026-09` branch and receive no changes. The Cranelift JIT is
Experimental in maturity but is an active product surface; its promotion
follows the criteria in [feature-matrix.md](feature-matrix.md).
