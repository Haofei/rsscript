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

## Current priority: product-contract convergence

The compiler-purity migration is complete. Its checked baseline is retained as
historical evidence, not as the active roadmap. The active work is to make the
existing Core path smaller, explicit, and usable without exposing migration
adapters.

1. Retire compatibility-only SDK and Provider paths behind an announced,
   tested removal plan. New embedding and Provider examples must use canonical
   `WireValue` APIs only.
2. Keep experiments one-way consumers of Core contracts. AOT, JIT, REIR, and
   self-hosting must not become default SDK, VM, CLI, or release dependencies.
3. Make one bytecode contract executable at a time. `rsscript.bytecode.v2` is a
   verifier-only prototype until an ADR-defined VM cutover; v1 remains the sole
   deployed execution schema.
4. Keep host deployment choices outside language validity and Artifact identity.
   The runner's host-selected profile performs admission, Provider installation,
   limits, and isolation; its non-secret identity is recorded in the response.
5. Maintain a short golden path: capture, build, verify, inspect, default
   isolated run, and machine-readable execution report.
6. Promote any feature only through the maturity matrix and measured workloads;
   do not add syntax while a Core row remains partial.

## Frozen scope

Until the priorities above are complete, do not expand language syntax, public
intrinsics, JIT tiers or speculation, the C backend, full self-host bootstrap,
package publishing, native plugin surface, or language-level policy. Correctness,
security boundary, maintenance, and measured-regression fixes remain allowed.

Rust AOT and Cranelift JIT stay Experimental. REIR stays an Integration.
Self-hosting stays Research. Promotion follows the criteria in
[feature-matrix.md](feature-matrix.md).
