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
5. Preserve the one-way experimental boundary. JIT, Rust AOT, REIR, and
   self-hosting accept only correctness, security, dependency, and regression
   maintenance; they do not gain roadmap feature work or become default SDK,
   VM, or CLI dependencies.
6. Preserve the current repository and workspace shape: do not split the
   repository, delete backends, or reorder the Cargo workspace as part of this
   work. Keep `rsscript.bytecode.v1`, host deployment boundaries, and the
   capture/build/verify/inspect/default-isolated-run golden path intact.

## Frozen scope

Until the priorities above are complete, do not expand language syntax, public
intrinsics, JIT tiers or speculation, the C backend, full self-host bootstrap,
package publishing, native plugin surface, or language-level policy. Correctness,
security boundary, maintenance, and measured-regression fixes remain allowed.

Rust AOT and Cranelift JIT stay Experimental. REIR stays an Integration.
Self-hosting stays Research. For these four surfaces, allowed changes are
limited to correctness, security, dependency, and regression maintenance;
promotion follows the criteria in [feature-matrix.md](feature-matrix.md).
