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
7. Published the strict execution-report contract, added a reusable Provider
   Conformance Kit for all official Providers, and added property/fuzz coverage
   for ownership, retention, resource handles, Artifact bytes, bindings, and
   report consumers.
8. Established a versioned Core SLO/reporting gate and a three-platform release
   dry-run with checksums, provenance, and explicit pre-1.0 SDK distribution.
9. Split the reference VM into `rsscript-vm`, established the independent owned
   `rsscript-exec-ir` model, and made HIR lowering a one-way compiler projection.
   Cargo metadata tests now reject any VM dependency on compiler, syntax,
   semantics, or lowering internals.

## Current priority: conformance and boundary hardening

1. Continue reducing compatibility-crate API and crate-wide lint exceptions.
2. Extend the query-level language-service cache only with measured editor
   workloads; formatting, lint, symbols, dependency discovery, and semantic
   diagnostics now invalidate independently.
3. Use the exact reachable-value live-memory metric and cumulative allocation
   quota together in workload tuning; extend the model only when a new VM-owned
   value kind is introduced. Provider-owned memory remains Provider telemetry.
4. Promote a feature only through the maturity matrix; do not add syntax while a
   Core row remains partial.

## Frozen scope

Until the priorities above are complete, do not expand language syntax, public
intrinsics, JIT tiers or speculation, the C backend, full self-host bootstrap,
package publishing, native plugin surface, or language-level policy. Correctness,
security boundary, maintenance, and measured-regression fixes remain allowed.

Rust AOT and Cranelift JIT stay Experimental. REIR stays an Integration.
Self-hosting stays Research. Promotion follows the criteria in
[feature-matrix.md](feature-matrix.md).
